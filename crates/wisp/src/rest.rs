//! The handlers `wisp-build` serves a `#[derive(Rest)]` type with, for
//! each method its `+server.rs` does not write itself:
//!
//! | Request | Answer |
//! |---|---|
//! | `GET /notes` | the rows, `?done=true&sort=-id&limit=20&after=40&fields=title` |
//! | `POST /notes` | 201 with the new row; an array makes several at once |
//! | `GET /notes/1` | the row, with an `etag`; 304 for `if-none-match` |
//! | `PUT /notes/1` | the row replaced (`if-match` guards against lost updates) |
//! | `PATCH /notes/1` | the members sent replace the row's |
//! | `DELETE /notes/1` | 204 |
//!
//! Each writes JSON while it holds the table's lock, so the type need not
//! be `Clone`, and a list makes one `String`. Hooks (`fn before_create` and
//! the like, in the same file) run outside the lock.

use crate::json::{self, FromJson, Value};
use crate::table::{Rows, row_json};
use crate::{Cx, Error, Json, Method, Response, Result, Row, Table};
use std::borrow::Cow;
use std::cmp::Ordering;

/// What JSON a field holds, for filters and sorting.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Text,
    Number,
    Bool,
    Other,
}

/// A type `#[derive(Rest)]` makes a JSON resource: its table, its fields,
/// and the environment variables holding the bearer tokens it needs.
pub trait Resource: Json + FromJson + 'static {
    /// Every request needs `Authorization: Bearer $KEY`.
    const KEY: Option<&'static str> = None;
    /// Writes (POST, PUT, PATCH, DELETE) need `Bearer $WRITE`, or `$KEY`
    /// when this is not set.
    const WRITE: Option<&'static str> = None;
    /// DELETE needs `Bearer $ADMIN`, or what other writes need when this is
    /// not set.
    const ADMIN: Option<&'static str> = None;
    /// Its fields, in order, with the JSON each holds.
    const FIELDS: &'static [(&'static str, Kind)];
    fn table() -> &'static Table<Self>;
    /// Writes field `name` as JSON; false when it has none of that name.
    fn field(&self, name: &str, out: &mut String) -> bool;
    /// Sets its `created_at` and `updated_at`, the ones it has: `old` is
    /// the row it replaces.
    fn stamp(&mut self, old: Option<&Self>) {
        let _ = old;
    }
}

/// The hooks a `+server.rs` defines for its `#[derive(Rest)]` type: plain
/// functions named for when they run, which `wisp-build` adapts to these.
/// A `before_` hook's error stops the change; an `after_` hook runs once it
/// is saved.
pub struct Hooks<T> {
    pub before_create: Option<fn(&mut Cx, &mut T) -> Result>,
    pub before_update: Option<fn(&mut Cx, u64, &mut T) -> Result>,
    pub before_delete: Option<fn(&mut Cx, &Row<T>) -> Result>,
    pub after_create: Option<fn(&mut Cx, &Row<T>) -> Result>,
    pub after_update: Option<fn(&mut Cx, &Row<T>) -> Result>,
    pub after_delete: Option<fn(&mut Cx, &Row<T>) -> Result>,
}

impl<T> Hooks<T> {
    pub const NONE: Hooks<T> = Hooks {
        before_create: None,
        before_update: None,
        before_delete: None,
        after_create: None,
        after_update: None,
        after_delete: None,
    };
}

fn missing() -> Error {
    Error::new(404, "Not Found")
}

/// 401 unless the request has the token the method needs.
fn guard<T: Resource>(cx: &Cx) -> Result {
    let key = match cx.method {
        Method::Delete => T::ADMIN.or(T::WRITE).or(T::KEY),
        _ if cx.writes() => T::WRITE.or(T::KEY),
        _ => T::KEY,
    };
    key.map_or(Ok(()), |k| cx.need_bearer(k))
}

fn id(cx: &Cx) -> Result<u64> {
    crate::input::required(cx, "id")
}

/// A field of `T` by the name a request gave it.
fn field<T: Resource>(name: &str) -> Result<(&'static str, Kind)> {
    T::FIELDS
        .iter()
        .find(|(f, _)| *f == name)
        .copied()
        .ok_or_else(|| {
            let known: Vec<&str> = T::FIELDS.iter().map(|(f, _)| *f).collect();
            Error::new(
                400,
                format!(
                    "There is no field `{name}`; the fields are: {}",
                    known.join(", ")
                ),
            )
            .with_code("unknown_field")
        })
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Op {
    Eq,
    Ne,
    Gt,
    Gte,
    Lt,
    Lte,
    Has,
}

struct Filter<'a> {
    field: &'static str,
    kind: Kind,
    op: Op,
    want: Cow<'a, str>,
}

/// What a GET asks for beyond the rows themselves.
#[derive(Default)]
struct View<'a> {
    filters: Vec<Filter<'a>>,
    /// Fields to sort by, each descending or not.
    sort: Vec<(&'static str, Kind, bool)>,
    /// Only these fields (and `id`).
    fields: Option<Vec<&'static str>>,
    limit: Option<usize>,
    offset: usize,
    after: Option<u64>,
}

/// The query's `limit offset after sort fields`, and a filter for each
/// other parameter: `done=true`, `title.has=tea`, `n.gte=3`. A route
/// parameter (other than `id`) named for a field is a filter too, which is
/// how `users/[user]/notes` holds each user's notes.
fn view<'a, T: Resource>(cx: &'a Cx, list: bool) -> Result<View<'a>> {
    let mut v = View::default();
    let bad = |name: &str, what: &str| {
        Err(Error::new(400, format!("query parameter `{name}`: {what}")).with_code("bad_query"))
    };
    for (name, want) in cx.params() {
        if name != "id"
            && let Ok((field, kind)) = field::<T>(name)
        {
            v.filters.push(Filter {
                field,
                kind,
                op: Op::Eq,
                want: Cow::Borrowed(want),
            });
        }
    }
    for (key, value) in cx.query_pairs() {
        match &*key {
            "fields" => {
                let mut fs = Vec::new();
                for f in value
                    .split(',')
                    .map(str::trim)
                    .filter(|f| !f.is_empty() && *f != "id")
                {
                    fs.push(field::<T>(f)?.0);
                }
                v.fields = Some(fs);
            }
            _ if !list => {}
            "limit" => match value.parse() {
                Ok(n) => v.limit = Some(n),
                Err(_) => return bad("limit", "expected a whole number"),
            },
            "offset" => match value.parse() {
                Ok(n) => v.offset = n,
                Err(_) => return bad("offset", "expected a whole number"),
            },
            "after" => match value.parse() {
                Ok(n) => v.after = Some(n),
                Err(_) => return bad("after", "expected an id"),
            },
            "sort" => {
                for s in value.split(',').map(str::trim).filter(|s| !s.is_empty()) {
                    let (name, desc) = match s.strip_prefix('-') {
                        Some(n) => (n, true),
                        None => (s, false),
                    };
                    if name == "id" {
                        v.sort.push(("id", Kind::Number, desc));
                    } else {
                        let (f, kind) = field::<T>(name)?;
                        v.sort.push((f, kind, desc));
                    }
                }
            }
            key => {
                let (name, op) = match key.rsplit_once('.') {
                    None => (key, Op::Eq),
                    Some((n, op)) => (
                        n,
                        match op {
                            "ne" => Op::Ne,
                            "gt" => Op::Gt,
                            "gte" => Op::Gte,
                            "lt" => Op::Lt,
                            "lte" => Op::Lte,
                            "has" => Op::Has,
                            _ => {
                                return bad(
                                    key,
                                    "filters are name, name.ne, .gt, .gte, .lt, .lte and .has",
                                );
                            }
                        },
                    ),
                };
                let (field, kind) = field::<T>(name)?;
                v.filters.push(Filter {
                    field,
                    kind,
                    op,
                    want: value,
                });
            }
        }
    }
    if v.after.is_some() && !v.sort.is_empty() {
        return bad(
            "after",
            "pages through rows in id order; with `sort`, use `offset`",
        );
    }
    Ok(v)
}

/// The text of a field's JSON: a string's contents, anything else as is.
fn text(json: &str) -> Cow<'_, str> {
    match json.strip_prefix('"').and_then(|s| s.strip_suffix('"')) {
        Some(s) if !s.contains('\\') => Cow::Borrowed(s),
        Some(_) => match json::parse(json) {
            Ok(Value::String(s)) => Cow::Owned(s),
            _ => Cow::Borrowed(json),
        },
        None => Cow::Borrowed(json),
    }
}

fn compare(kind: Kind, a: &str, b: &str) -> Ordering {
    if kind == Kind::Number
        && let (Ok(x), Ok(y)) = (a.parse::<f64>(), b.parse::<f64>())
    {
        return x.partial_cmp(&y).unwrap_or(Ordering::Equal);
    }
    a.cmp(b)
}

impl Filter<'_> {
    /// Whether a field whose JSON is `json` passes.
    fn passes(&self, json: &str) -> bool {
        let got = text(json);
        let ord = || compare(self.kind, &got, &self.want);
        match self.op {
            Op::Eq => ord() == Ordering::Equal,
            Op::Ne => ord() != Ordering::Equal,
            Op::Gt => ord() == Ordering::Greater,
            Op::Gte => ord() != Ordering::Less,
            Op::Lt => ord() == Ordering::Less,
            Op::Lte => ord() != Ordering::Greater,
            // A list has the item; text has it in any case.
            Op::Has => match json::parse(json) {
                Ok(Value::Array(items)) => items.iter().any(|i| match i {
                    Value::String(s) => *s == self.want,
                    other => json::to_json(other) == self.want,
                }),
                _ => got.to_lowercase().contains(&self.want.to_lowercase()),
            },
        }
    }
}

impl View<'_> {
    fn keeps<T: Resource>(&self, v: &T, scratch: &mut String) -> bool {
        self.filters.iter().all(|f| {
            scratch.clear();
            v.field(f.field, scratch);
            f.passes(scratch)
        })
    }

    /// The row as JSON, all of it or the fields asked for.
    fn write<T: Resource>(&self, out: &mut String, id: u64, v: &T) {
        let Some(fields) = &self.fields else {
            return row_json(out, id, v);
        };
        out.push_str("{\"id\":");
        id.json(out);
        for f in fields {
            out.push(',');
            f.json(out);
            out.push(':');
            v.field(f, out);
        }
        out.push('}');
    }
}

/// A strong ETag for a JSON body: FNV-1a of it.
fn etag(body: &str) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in body.as_bytes() {
        h = (h ^ b as u64).wrapping_mul(0x0000_0100_0000_01b3);
    }
    let mut tag = String::with_capacity(18);
    tag.push('"');
    for shift in (0..16).rev() {
        tag.push(char::from(
            b"0123456789abcdef"[(h >> (shift * 4)) as usize & 15],
        ));
    }
    tag.push('"');
    tag
}

/// Whether `header` (`if-match` or `if-none-match`) names `tag`.
fn names(header: &str, tag: &str) -> bool {
    header
        .split(',')
        .map(|t| t.trim().trim_start_matches("W/"))
        .any(|t| t == "*" || t == tag)
}

/// 200 with the JSON and its ETag, or 304 when the client has it.
fn tagged(cx: &Cx, body: String) -> Response {
    let tag = etag(&body);
    if cx.header("if-none-match").is_some_and(|h| names(h, &tag)) {
        return Response::empty(304).with_header("etag", tag);
    }
    Response::json(body).with_header("etag", tag)
}

/// 412 unless an `if-match` the request sends names the row as it is.
fn check_match<T: Json>(cx: &Cx, id: u64, v: &T) -> Result {
    let Some(want) = cx.header("if-match") else {
        return Ok(());
    };
    let mut now = String::new();
    row_json(&mut now, id, v);
    if names(want, &etag(&now)) {
        return Ok(());
    }
    Err(Error::new(412, "The row has changed since it was read").with_code("changed"))
}

/// `GET /notes`: the rows that pass the filters, sorted and paged as the
/// query says, with their count in `x-total-count` and the next page in
/// `link`. `accept: application/x-ndjson` gets a row per line.
pub fn list<T: Resource>(cx: &mut Cx, _: &Hooks<T>) -> Result<Response> {
    guard::<T>(cx)?;
    let cx = &*cx;
    let view = view::<T>(cx, true)?;
    let lines = cx
        .header("accept")
        .is_some_and(|a| a.contains("application/x-ndjson"));
    let (open, sep, close) = if lines {
        ("", "\n", "\n")
    } else {
        ("[", ",", "]")
    };
    let rows = T::table().rows();
    let mut out = String::with_capacity(64 * rows.map.len().min(1024) + 2);
    out.push_str(open);
    let mut scratch = String::new();
    let limit = view.limit.unwrap_or(usize::MAX);
    // Rows that pass, those of them past the cursor, those shown.
    let (mut total, mut seen, mut shown, mut last) = (0, 0, 0, None);
    let mut put = |out: &mut String, id: u64, v: &T| {
        if shown > 0 {
            out.push_str(sep);
        }
        view.write(out, id, v);
        shown += 1;
        last = Some(id);
    };
    if view.sort.is_empty() {
        let after = view.after.unwrap_or(0);
        for (&id, v) in &rows.map {
            if view.keeps(v, &mut scratch) {
                total += 1;
                if id > after {
                    seen += 1;
                    if seen > view.offset && seen - view.offset <= limit {
                        put(&mut out, id, v);
                    }
                }
            }
        }
    } else {
        let mut found: Vec<(Vec<String>, u64, &T)> = Vec::new();
        for (&id, v) in &rows.map {
            if view.keeps(v, &mut scratch) {
                let keys = view
                    .sort
                    .iter()
                    .map(|(f, _, _)| {
                        let mut k = String::new();
                        match *f {
                            "id" => id.json(&mut k),
                            f => {
                                v.field(f, &mut k);
                            }
                        }
                        text(&k).into_owned()
                    })
                    .collect();
                found.push((keys, id, v));
            }
        }
        found.sort_by(|a, b| {
            for (k, (_, kind, desc)) in view.sort.iter().enumerate() {
                let o = compare(*kind, &a.0[k], &b.0[k]);
                if o != Ordering::Equal {
                    return if *desc { o.reverse() } else { o };
                }
            }
            a.1.cmp(&b.1)
        });
        (total, seen) = (found.len(), found.len());
        for (_, id, v) in found.iter().skip(view.offset).take(limit) {
            put(&mut out, *id, v);
        }
    }
    drop(rows);
    if shown > 0 || !lines {
        out.push_str(close);
    }
    let mut res = if lines {
        Response::new("application/x-ndjson", out)
    } else {
        tagged(cx, out)
    };
    res = res.with_header("x-total-count", total.to_string());
    if shown > 0 && seen > view.offset + shown {
        res = res.with_header("link", next(cx, &view, last, shown));
    }
    Ok(res)
}

/// `<…?after=5&limit=20>; rel="next"`: the query with the page moved on.
fn next(cx: &Cx, view: &View, last: Option<u64>, shown: usize) -> String {
    let keep: Vec<String> = cx
        .query_string()
        .split('&')
        .filter(|p| {
            let k = p.split('=').next().unwrap_or("");
            !p.is_empty() && k != "after" && k != "offset"
        })
        .map(str::to_string)
        .collect();
    let mut q = keep.join("&");
    if !q.is_empty() {
        q.push('&');
    }
    // In id order the last row shown is the cursor; sorted, the count is.
    match (view.sort.is_empty(), last) {
        (true, Some(id)) => q.push_str(&format!("after={id}")),
        _ => q.push_str(&format!("offset={}", view.offset + shown)),
    }
    format!("<{}?{q}>; rel=\"next\"", cx.path())
}

/// `GET /notes/[id]`, with `?fields=` as for the list.
pub fn get<T: Resource>(cx: &mut Cx, _: &Hooks<T>) -> Result<Response> {
    guard::<T>(cx)?;
    let cx = &*cx;
    let view = view::<T>(cx, false)?;
    let id = id(cx)?;
    let rows = T::table().rows();
    let v = one(&rows, id, &view)?;
    let mut out = String::with_capacity(128);
    view.write(&mut out, id, v);
    drop(rows);
    Ok(tagged(cx, out))
}

/// The row, if it passes the route's filters (`users/[user]/notes/[id]`).
fn one<'r, T: Resource>(rows: &'r Rows<T>, id: u64, view: &View) -> Result<&'r T> {
    let v = rows.map.get(&id).ok_or_else(missing)?;
    if view.keeps(v, &mut String::new()) {
        Ok(v)
    } else {
        Err(missing())
    }
}

/// The request's body as JSON, with the route's parameters that name
/// fields set in each object: the `user` of `users/[user]/notes`.
fn body<T: Resource>(cx: &Cx) -> Result<Value> {
    let mut v: Value = crate::input::body(cx)?;
    let set = |o: &mut Value| {
        let Value::Object(members) = o else { return };
        for (name, value) in cx.params() {
            let Some((f, kind)) = T::FIELDS.iter().find(|(f, _)| *f == name) else {
                continue;
            };
            let value = match kind {
                Kind::Number => Value::Number(value.to_string()),
                Kind::Bool => Value::Bool(value == "true"),
                _ => Value::String(value.to_string()),
            };
            members.retain(|(k, _)| k != f);
            members.push((f.to_string(), value));
        }
    };
    match &mut v {
        Value::Array(items) => items.iter_mut().for_each(set),
        o => set(o),
    }
    Ok(v)
}

/// A row that was just written, read back from its JSON for an `after_`
/// hook (the type need not be `Clone`).
fn reread<T: Resource>(json: &str) -> Result<Row<T>> {
    json::from_json(json.as_bytes())
}

/// `POST /notes`: 201 with the new row and its `Location`. An array makes
/// a row of each, all or none, and answers with them.
pub fn create<T: Resource>(cx: &mut Cx, hooks: &Hooks<T>) -> Result<Response> {
    guard::<T>(cx)?;
    let sent = body::<T>(cx)?;
    let many = matches!(sent, Value::Array(_));
    let mut values: Vec<T> = if many {
        json::from_value(&sent)?
    } else {
        vec![json::from_value(&sent)?]
    };
    if let Some(h) = hooks.before_create {
        for v in &mut values {
            h(cx, v)?;
        }
    }
    let table = T::table();
    let mut rows = table.rows();
    let mut out = String::with_capacity(128 * values.len());
    let mut made = Vec::with_capacity(values.len());
    if many {
        out.push('[');
    }
    for mut v in values {
        v.stamp(None);
        let id = table.next_id(&mut rows);
        let json = table.encode(&rows, &v);
        table.write(&mut rows, id, Some(&json));
        if !made.is_empty() {
            out.push(',');
        }
        let at = out.len();
        row_json(&mut out, id, &v);
        made.push((id, at, out.len()));
        rows.map.insert(id, v);
    }
    drop(rows);
    if many {
        out.push(']');
    }
    if let Some(h) = hooks.after_create {
        for &(_, a, b) in &made {
            h(cx, &reread(&out[a..b])?)?;
        }
    }
    if many {
        return Ok(Response::json(out).with_status(201));
    }
    let at = format!("{}/{}", cx.path().trim_end_matches('/'), made[0].0);
    Ok(tagged(cx, out).with_status(201).with_header("location", at))
}

/// `PUT /notes/[id]`: the whole value, replaced.
pub fn put<T: Resource>(cx: &mut Cx, hooks: &Hooks<T>) -> Result<Response> {
    guard::<T>(cx)?;
    let id = id(cx)?;
    let mut v: T = json::from_value(&body::<T>(cx)?)?;
    {
        let view = view::<T>(cx, false)?;
        let rows = T::table().rows();
        check_match(cx, id, one(&rows, id, &view)?)?;
    }
    if let Some(h) = hooks.before_update {
        h(cx, id, &mut v)?;
    }
    replace(cx, hooks, id, v, None)
}

/// Writes `v` in place of row `id` (which must still be as `was`, when
/// given: its JSON as the change was made from it), answering with it.
fn replace<T: Resource>(
    cx: &mut Cx,
    hooks: &Hooks<T>,
    id: u64,
    mut v: T,
    was: Option<&str>,
) -> Result<Response> {
    let table = T::table();
    let mut rows = table.rows();
    let old = one(&rows, id, &view::<T>(cx, false)?)?;
    if was.is_some_and(|w| json::to_json(old) != w) {
        return Err(
            Error::new(409, "The row changed while it was being updated").with_code("conflict"),
        );
    }
    check_match(cx, id, old)?;
    v.stamp(Some(old));
    let json = table.encode(&rows, &v);
    table.write(&mut rows, id, Some(&json));
    let mut out = String::with_capacity(128);
    row_json(&mut out, id, &v);
    rows.map.insert(id, v);
    drop(rows);
    if let Some(h) = hooks.after_update {
        h(cx, &reread(&out)?)?;
    }
    Ok(tagged(cx, out))
}

/// `PATCH /notes/[id]`: the members the body sends replace the row's (a
/// `null` empties an `Option`), and the result must pass the type's
/// checks (a 422 names what does not).
pub fn patch<T: Resource>(cx: &mut Cx, hooks: &Hooks<T>) -> Result<Response> {
    guard::<T>(cx)?;
    let id = id(cx)?;
    let Value::Object(sent) = body::<T>(cx)? else {
        return Err(Error::invalid("body", "expected an object"));
    };
    let was = {
        let view = view::<T>(cx, false)?;
        let rows = T::table().rows();
        let old = one(&rows, id, &view)?;
        check_match(cx, id, old)?;
        json::to_json(old)
    };
    let Ok(Value::Object(mut members)) = json::parse(&was) else {
        return Err(Error::new(500, "The row is not a JSON object"));
    };
    for (k, x) in sent {
        if k == "id" {
            continue;
        }
        match members.iter_mut().find(|(m, _)| *m == k) {
            Some(slot) => slot.1 = x,
            None => members.push((k, x)),
        }
    }
    let mut v: T = json::from_value(&Value::Object(members))?;
    if let Some(h) = hooks.before_update {
        h(cx, id, &mut v)?;
    }
    replace(cx, hooks, id, v, Some(&was))
}

/// `DELETE /notes/[id]`: 204.
pub fn delete<T: Resource>(cx: &mut Cx, hooks: &Hooks<T>) -> Result<Response> {
    guard::<T>(cx)?;
    let id = id(cx)?;
    let table = T::table();
    let view = view::<T>(cx, false)?;
    let hooked = hooks.before_delete.is_some() || hooks.after_delete.is_some();
    let row = {
        let rows = table.rows();
        let v = one(&rows, id, &view)?;
        check_match(cx, id, v)?;
        let mut out = String::new();
        if hooked {
            row_json(&mut out, id, v);
        }
        out
    };
    let row = if hooked {
        Some(reread::<T>(&row)?)
    } else {
        None
    };
    if let (Some(h), Some(row)) = (hooks.before_delete, &row) {
        h(cx, row)?;
    }
    {
        table.delete(&mut table.rows(), id).ok_or_else(missing)?;
    }
    if let (Some(h), Some(row)) = (hooks.after_delete, &row) {
        h(cx, row)?;
    }
    Ok(Response::empty(204))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filters_compare_by_kind() {
        let f = |kind, op, want: &'static str| Filter {
            field: "x",
            kind,
            op,
            want: Cow::Borrowed(want),
        };
        assert!(f(Kind::Text, Op::Eq, "Tea").passes("\"Tea\""));
        assert!(f(Kind::Text, Op::Eq, "a<b").passes("\"a\\u003cb\""));
        assert!(!f(Kind::Text, Op::Eq, "tea").passes("\"Tea\""));
        assert!(f(Kind::Text, Op::Has, "EA").passes("\"Tea\""));
        assert!(f(Kind::Other, Op::Has, "home").passes("[\"home\",\"work\"]"));
        assert!(!f(Kind::Other, Op::Has, "hom").passes("[\"home\"]"));
        assert!(f(Kind::Number, Op::Gt, "9").passes("10"), "as numbers");
        assert!(f(Kind::Text, Op::Lt, "9").passes("\"10\""), "as text");
        assert!(f(Kind::Number, Op::Eq, "3").passes("3.0"));
        assert!(f(Kind::Bool, Op::Eq, "true").passes("true"));
        assert!(f(Kind::Bool, Op::Ne, "true").passes("false"));
        assert!(f(Kind::Text, Op::Gte, "2026-01-01").passes("\"2026-09-29T10:00:00Z\""));
    }

    #[test]
    fn etags_and_their_headers() {
        let t = etag("[]");
        assert!(t.starts_with('"') && t.len() == 18);
        assert_ne!(t, etag("[1]"));
        assert!(names(&format!("\"x\", W/{t}"), &t));
        assert!(names("*", &t) && !names("\"x\"", &t));
    }
}
