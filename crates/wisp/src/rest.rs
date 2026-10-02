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
//! A read writes its JSON under the table's read lock, which reads share,
//! so the type need not be `Clone`, and a list makes one `String`. A change
//! writes its row's JSON once, for the store and the answer both. Hooks
//! (`fn before_create` and the like, in the same file) run outside the lock.

use crate::json::{self, FromJson, Value};
use crate::table::{Rows, row_json, splice};
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
    op: Op,
    want: Cow<'a, str>,
    /// `want` as a number, for a number field: then a field's value that
    /// is a number too compares as one.
    number: Option<f64>,
    /// `want` in lower case, for `.has`.
    lower: String,
}

impl<'a> Filter<'a> {
    fn new(field: &'static str, kind: Kind, op: Op, want: Cow<'a, str>) -> Filter<'a> {
        Filter {
            field,
            op,
            number: (kind == Kind::Number).then(|| want.parse().ok()).flatten(),
            lower: if op == Op::Has {
                want.to_lowercase()
            } else {
                String::new()
            },
            want,
        }
    }
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
            v.filters
                .push(Filter::new(field, kind, Op::Eq, Cow::Borrowed(want)));
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
                v.filters.push(Filter::new(field, kind, op, value));
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

/// A field's value to sort by: its text, and the number it is, for a
/// number field. Numbers compare as numbers, anything else as text.
struct Key {
    text: String,
    number: Option<f64>,
}

impl Key {
    fn new(kind: Kind, json: &str) -> Key {
        let text = text(json).into_owned();
        let number = (kind == Kind::Number).then(|| text.parse().ok()).flatten();
        Key { text, number }
    }

    fn cmp(&self, other: &Key) -> Ordering {
        match (self.number, other.number) {
            (Some(x), Some(y)) => x.partial_cmp(&y).unwrap_or(Ordering::Equal),
            _ => self.text.cmp(&other.text),
        }
    }
}

impl Filter<'_> {
    /// Whether a field whose JSON is `json` passes.
    fn passes(&self, json: &str) -> bool {
        let got = text(json);
        let ord = || match (self.number, self.number.and(got.parse::<f64>().ok())) {
            (Some(y), Some(x)) => x.partial_cmp(&y).unwrap_or(Ordering::Equal),
            _ => (*got).cmp(&*self.want),
        };
        match self.op {
            Op::Eq => ord() == Ordering::Equal,
            Op::Ne => ord() != Ordering::Equal,
            Op::Gt => ord() == Ordering::Greater,
            Op::Gte => ord() != Ordering::Less,
            Op::Lt => ord() == Ordering::Less,
            Op::Lte => ord() != Ordering::Greater,
            // A list has the item; text has it in any case.
            Op::Has => match json.starts_with('[').then(|| json::parse(json)) {
                Some(Ok(Value::Array(items))) => items.iter().any(|i| match i {
                    Value::String(s) => *s == self.want,
                    other => json::to_json(other) == self.want,
                }),
                _ => got.to_lowercase().contains(&self.lower),
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

/// A 64-bit hash of `parts`, eight bytes at a step, for ETags and
/// idempotency keys: quick, and not for secrets. Each step is a bijection
/// of the state, so bodies of one length that differ never share a hash.
pub(crate) fn hash(parts: &[&[u8]]) -> u64 {
    const K: u64 = 0x517c_c1b7_2722_0a95;
    let step = |h: u64, w: u64| (h.rotate_left(5) ^ w).wrapping_mul(K);
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for p in parts {
        let (words, rest) = p.as_chunks::<8>();
        for w in words {
            h = step(h, u64::from_le_bytes(*w));
        }
        let mut last = [0; 8];
        last[..rest.len()].copy_from_slice(rest);
        h = step(step(h, u64::from_le_bytes(last)), p.len() as u64);
    }
    h ^ h >> 32
}

/// A strong ETag for a body (JSON, or a page `CACHE` keeps): its [`hash`].
pub(crate) fn etag(body: impl AsRef<[u8]>) -> String {
    let h = hash(&[body.as_ref()]);
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

/// Whether `if-none-match` names `tag`: weakly, so `W/"x"` is `"x"`.
pub(crate) fn names(header: &str, tag: &str) -> bool {
    header
        .split(',')
        .map(|t| t.trim().trim_start_matches("W/"))
        .any(|t| t == "*" || t == tag)
}

/// Whether `if-match` names `tag`: strongly (RFC 9110 13.1.1), so a weak
/// `W/"x"` names nothing, as it promises no byte is the same.
fn names_strongly(header: &str, tag: &str) -> bool {
    header
        .split(',')
        .map(str::trim)
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

/// 412 unless an `if-match` the request sends names the row as it is:
/// `json`, its value's JSON, or `v` written when that is not at hand.
fn check_match<T: Json>(cx: &Cx, id: u64, v: &T, json: Option<&str>) -> Result {
    let Some(want) = cx.header("if-match") else {
        return Ok(());
    };
    let mut now = String::new();
    match json {
        Some(j) => splice(&mut now, id, j),
        None => row_json(&mut now, id, v),
    }
    if names_strongly(want, &etag(&now)) {
        return Ok(());
    }
    Err(Error::new(412, "The row has changed since it was read").with_code("changed"))
}

/// Whether a list is asked for a row per line (`accept:
/// application/x-ndjson`) rather than as a JSON array. What `CACHE` keeps
/// of a list is kept apart by it (see `bake::cached_by_accept`).
pub(crate) fn lines(cx: &Cx) -> bool {
    cx.header("accept")
        .is_some_and(|a| a.contains("application/x-ndjson"))
}

/// `GET /notes`: the rows that pass the filters, sorted and paged as the
/// query says, with their count in `x-total-count` and the next page in
/// `link`. `accept: application/x-ndjson` gets a row per line.
pub fn list<T: Resource>(cx: &mut Cx, _: &Hooks<T>) -> Result<Response> {
    guard::<T>(cx)?;
    // JSON or NDJSON by `accept`: caches keep the two apart.
    cx.put("vary", Cow::Borrowed("accept"));
    let cx = &*cx;
    let view = view::<T>(cx, true)?;
    let lines = lines(cx);
    let (open, sep, close) = if lines {
        ("", "\n", "\n")
    } else {
        ("[", ",", "]")
    };
    let rows = T::table().read();
    let mut out = String::with_capacity(64 * rows.map.len().min(1024) + 2);
    out.push_str(open);
    let mut scratch = String::new();
    let limit = view.limit.unwrap_or(usize::MAX);
    let (mut shown, mut last) = (0, None);
    let mut put = |out: &mut String, id: u64, v: &T| {
        if shown > 0 {
            out.push_str(sep);
        }
        view.write(out, id, v);
        shown += 1;
        last = Some(id);
    };
    // Rows that pass, and whether there are more past those shown.
    let (total, more);
    if view.sort.is_empty() && view.filters.is_empty() {
        let after = std::ops::Bound::Excluded(view.after.unwrap_or(0));
        let mut rest = rows.map.range((after, std::ops::Bound::Unbounded));
        for (&id, v) in rest.by_ref().skip(view.offset).take(limit) {
            put(&mut out, id, v);
        }
        (total, more) = (rows.map.len(), rest.next().is_some());
    } else if view.sort.is_empty() {
        let after = view.after.unwrap_or(0);
        // Rows that pass, and those of them past the cursor.
        let (mut passed, mut seen) = (0, 0);
        for (&id, v) in &rows.map {
            if view.keeps(v, &mut scratch) {
                passed += 1;
                if id > after {
                    seen += 1;
                    if seen > view.offset && seen - view.offset <= limit {
                        put(&mut out, id, v);
                    }
                }
            }
        }
        (total, more) = (passed, seen > view.offset + shown);
    } else {
        let mut found: Vec<(Vec<Key>, u64, &T)> = Vec::new();
        for (&id, v) in &rows.map {
            if view.keeps(v, &mut scratch) {
                let keys = view
                    .sort
                    .iter()
                    .map(|&(f, kind, _)| {
                        scratch.clear();
                        match f {
                            "id" => id.json(&mut scratch),
                            f => {
                                v.field(f, &mut scratch);
                            }
                        }
                        Key::new(kind, &scratch)
                    })
                    .collect();
                found.push((keys, id, v));
            }
        }
        found.sort_by(|a, b| {
            for (k, &(_, _, desc)) in view.sort.iter().enumerate() {
                let o = a.0[k].cmp(&b.0[k]);
                if o != Ordering::Equal {
                    return if desc { o.reverse() } else { o };
                }
            }
            a.1.cmp(&b.1)
        });
        for (_, id, v) in found.iter().skip(view.offset).take(limit) {
            put(&mut out, *id, v);
        }
        (total, more) = (found.len(), found.len() > view.offset + shown);
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
    if shown > 0 && more {
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
    let rows = T::table().read();
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

/// Row `id`, read back from its value's JSON for a hook (the type need not
/// be `Clone`).
fn reread<T: Resource>(id: u64, json: &str) -> Result<Row<T>> {
    let value = json::from_json(json.as_bytes())?;
    Ok(Row { id, value })
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
    // Each value's JSON, written once for the store and the answer.
    let values: Vec<(T, String)> = values
        .into_iter()
        .map(|mut v| {
            v.stamp(None);
            let json = json::to_json(&v);
            (v, json)
        })
        .collect();
    let table = T::table();
    let mut rows = table.write();
    let mut made = Vec::with_capacity(values.len());
    for (v, json) in values {
        let id = table.next_id(&mut rows);
        rows.map.insert(id, v);
        made.push((id, json));
    }
    // All or none: a store that fails takes them out again.
    table.save_many(&mut rows, &made)?;
    drop(rows);
    let mut out = String::with_capacity(made.iter().map(|(_, j)| j.len() + 24).sum::<usize>() + 2);
    out.push_str(if many { "[" } else { "" });
    for (k, (id, json)) in made.iter().enumerate() {
        out.push_str(if k > 0 { "," } else { "" });
        splice(&mut out, *id, json);
    }
    if let Some(h) = hooks.after_create {
        for (id, json) in &made {
            h(cx, &reread(*id, json)?)?;
        }
    }
    if many {
        out.push(']');
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
    if let Some(h) = hooks.before_update {
        // A 404 before the hook, for a row there is not.
        one(&T::table().read(), id, &view::<T>(cx, false)?)?;
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
    let view = view::<T>(cx, false)?;
    let mut rows = table.write();
    let old = one(&rows, id, &view)?;
    if was.is_some_and(|w| json::to_json(old) != w) {
        return Err(
            Error::new(409, "The row changed while it was being updated").with_code("conflict"),
        );
    }
    check_match(cx, id, old, was)?;
    v.stamp(Some(old));
    let json = json::to_json(&v);
    table.save(&mut rows, id, Some(&json));
    rows.map.insert(id, v);
    drop(rows);
    let mut out = String::with_capacity(json.len() + 24);
    splice(&mut out, id, &json);
    if let Some(h) = hooks.after_update {
        h(cx, &reread(id, &json)?)?;
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
    let was = json::to_json(one(&T::table().read(), id, &view::<T>(cx, false)?)?);
    let Ok(Value::Object(mut members)) = json::parse(&was) else {
        return Err(Error::new(500, "The row is not a JSON object"));
    };
    // Only the type's fields: the rest would be ignored, and a body of
    // many other names would be a search of the row's for each.
    for (k, x) in sent {
        if k == "id" || !T::FIELDS.iter().any(|(f, _)| *f == k) {
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
    // The row as it was, for the hooks.
    let hooked = hooks.before_delete.is_some() || hooks.after_delete.is_some();
    let row = hooked
        .then(|| {
            let json = json::to_json(one(&table.read(), id, &view::<T>(cx, false)?)?);
            reread::<T>(id, &json)
        })
        .transpose()?;
    if let (Some(h), Some(row)) = (hooks.before_delete, &row) {
        h(cx, row)?;
    }
    let view = view::<T>(cx, false)?;
    let mut rows = table.write();
    check_match(cx, id, one(&rows, id, &view)?, None)?;
    table.delete(&mut rows, id);
    drop(rows);
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
        let f = |kind, op, want: &'static str| Filter::new("x", kind, op, Cow::Borrowed(want));
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
        assert_ne!(etag("[1234567]"), etag("[1234568]"));
        assert_ne!(etag("[123456]\0"), etag("[123456]"), "the length counts");
        assert_ne!(
            hash(&[b"ab", b"c"]),
            hash(&[b"a", b"bc"]),
            "and each part's"
        );
        assert!(names(&format!("\"x\", W/{t}"), &t));
        assert!(names("*", &t) && !names("\"x\"", &t));
        assert!(names_strongly(&format!("\"x\", {t}"), &t) && names_strongly("*", &t));
        assert!(!names_strongly(&format!("W/{t}"), &t), "if-match is strong");
    }

    struct Item(String);

    impl Json for Item {
        fn json(&self, out: &mut String) {
            self.0.json(out);
        }
    }

    impl FromJson for Item {
        fn from_json(v: &Value, p: &mut json::Problems) -> Option<Item> {
            String::from_json(v, p).map(Item)
        }
    }

    static ITEMS: Table<Item> = Table::rest(Some("rest_bulk_all_or_none"), false);

    impl Resource for Item {
        const FIELDS: &'static [(&'static str, Kind)] = &[];
        fn table() -> &'static Table<Item> {
            &ITEMS
        }
        fn field(&self, _: &str, _: &mut String) -> bool {
            false
        }
    }

    /// A store that fails its third save, as a database that lost its
    /// connection part way would, and keeps the rest as a log.
    #[derive(Default)]
    struct Flaky {
        saves: std::sync::atomic::AtomicUsize,
        log: std::sync::Mutex<Vec<(u64, Option<String>)>>,
    }

    impl crate::Store for Flaky {
        fn load(&self, _: &str) -> Result<Vec<(u64, String)>> {
            let mut rows = std::collections::BTreeMap::new();
            for (id, json) in self.log.lock().unwrap().iter() {
                match json {
                    Some(j) => rows.insert(*id, j.clone()),
                    None => rows.remove(id),
                };
            }
            Ok(rows.into_iter().collect())
        }

        fn save(&self, _: &str, id: u64, json: Option<&str>) -> Result {
            if self
                .saves
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
                == 2
            {
                return Err(Error::new(500, "connection lost"));
            }
            self.log
                .lock()
                .unwrap()
                .push((id, json.map(str::to_string)));
            Ok(())
        }
    }

    #[test]
    fn a_bulk_create_is_all_or_none() {
        let flaky: &'static Flaky = Box::leak(Box::new(Flaky::default()));
        ITEMS.load_from(flaky);
        let body = r#"["a","b","c"]"#;
        let raw = format!(
            "POST /items HTTP/1.1\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n{body}",
            body.len()
        );
        let mut cx = Cx::for_test(&raw, &[]);
        let Err(err) = create::<Item>(&mut cx, &Hooks::NONE) else {
            panic!("a store that failed answered 201");
        };
        assert_eq!(err.status(), 500);
        assert!(
            crate::Store::load(flaky, "").unwrap().is_empty(),
            "the two saved are removed again"
        );
        assert_eq!(ITEMS.len(), 0, "and none are in memory");
        let mut cx = Cx::for_test(&raw, &[]);
        assert_eq!(create::<Item>(&mut cx, &Hooks::NONE).unwrap().status, 201);
        assert_eq!(crate::Store::load(flaky, "").unwrap().len(), 3);
        assert_eq!(ITEMS.len(), 3);
    }
}
