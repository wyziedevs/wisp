//! A handler's parameters other than `cx`: `fn load(slug: String, page:
//! Option<u32>)`, `#[action] fn add(text: String)`. The generated call reads
//! each by its name: a route parameter first, then the form a POST, PUT or
//! PATCH sends, then the URL's query. The type says how: `Option` when it
//! may be left out (or blank), `bool` for a checkbox, `Vec` for every value
//! of a repeated field, anything else `FromStr` when it must be there. A
//! JSON body's members are read the same way, and `body: T` reads all of it.

use crate::{Cx, Error, FromJson, Method, Result, Value};
use std::borrow::Cow;
use std::collections::HashMap;
use std::fmt::Display;
use std::str::FromStr;

/// Where a value was found, for the error that says it is wrong.
#[derive(Clone, Copy)]
enum From {
    Param,
    Form,
    Json,
    Query,
}

fn posts(cx: &Cx) -> bool {
    matches!(cx.method, Method::Post | Method::Put | Method::Patch)
}

fn find<'a>(cx: &'a Cx, name: &str) -> Option<(Cow<'a, str>, From)> {
    if let Some(v) = cx.route_param(name) {
        return Some((Cow::Borrowed(v), From::Param));
    }
    if posts(cx) {
        if let Some(v) = cx.form().get(name) {
            return Some((v, From::Form));
        }
        if let Some(v) = json_values(cx, name).next() {
            return Some((v, From::Json));
        }
    }
    cx.query(name).map(|v| (v, From::Query))
}

/// Whether the client asks for JSON rather than a page, by its `Accept`.
pub(crate) fn asks_json(cx: &Cx) -> bool {
    let accept = cx.header("accept").unwrap_or("");
    accept.contains("json") && !accept.contains("text/html")
}

/// What an action's error does: `invalid(..)` from a form shows the page
/// again, as a 422, for [`Cx::problem`] to tell; any other error, or one
/// for a client that sent or asks for JSON, is the error.
pub fn failed(cx: &mut Cx, e: Error) -> Result<()> {
    let json = is_json(cx) || asks_json(cx);
    if e.status() == 422 && !e.fields().is_empty() && !json {
        cx.fail(422, e);
        return Ok(());
    }
    Err(e)
}

impl Cx {
    /// The request's value `name`, found the way a handler's parameter of
    /// that name is: a route parameter, then the form a POST, PUT or PATCH
    /// sends (or its JSON body), then the query. It shows what was typed
    /// again: `<input name="email" value={cx.input("email")}>`.
    pub fn input(&self, name: &str) -> Option<Cow<'_, str>> {
        find(self, name).map(|(v, _)| v)
    }

    /// What is wrong with the field `name` when an action returned
    /// `invalid(name, problem)`, which renders the page again as a 422:
    /// `{#if let Some(p) = cx.problem("email")}<p>{p}</p>{/if}`.
    pub fn problem(&self, name: &str) -> Option<&str> {
        let e = self.get::<Error>()?;
        let (_, problem) = e.fields().iter().find(|(f, _)| f == name)?;
        Some(problem)
    }
}

/// Whether the request's body is JSON, by its `Content-Type`.
pub(crate) fn is_json(cx: &Cx) -> bool {
    let t = cx.mime().as_bytes();
    t.eq_ignore_ascii_case(b"application/json")
        || t.len() > 5 && t[t.len() - 5..].eq_ignore_ascii_case(b"+json")
}

/// The members `name` of a JSON object body, as the text a form would send:
/// a string as itself, a number or boolean as written, each item of an
/// array. `null` is not there. Borrowed from the parsed body.
fn json_values<'a>(cx: &'a Cx, name: &str) -> impl Iterator<Item = Cow<'a, str>> {
    let scalar = |v: &'a Value| match v {
        Value::String(s) | Value::Number(s) => Some(Cow::Borrowed(s.as_str())),
        Value::Bool(b) => Some(Cow::Borrowed(if *b { "true" } else { "false" })),
        _ => None,
    };
    let found = is_json(cx).then(|| cx.json_body()?.get(name)).flatten();
    let items = match found {
        Some(Value::Array(items)) => items.as_slice(),
        Some(v) => std::slice::from_ref(v),
        None => &[],
    };
    items.iter().filter_map(scalar)
}

/// `body: T`: the request's JSON body read as a `T` (see
/// [`crate::from_json`]). A body sent as another type is a 415.
pub fn body<T: FromJson>(cx: &Cx) -> Result<T> {
    if cx.header("content-type").is_some() && !is_json(cx) {
        return Err(Error::new(
            415,
            "Expected a JSON body, sent with Content-Type: application/json",
        ));
    }
    crate::from_json(cx.body())
}

fn parse<T: FromStr<Err: Display>>(name: &str, v: &str, from: From) -> Result<T> {
    v.parse().map_err(|e: T::Err| match from {
        // A path whose segment is not one of these: there is no such page.
        From::Param => Error::new(404, "Not Found"),
        // A field that is not one: the page again, the problem by it.
        From::Form | From::Json => Error::invalid(name, e.to_string()),
        From::Query => Error::new(400, format!("query parameter `{name}`: {e}")),
    })
}

/// `name: T`: missing is a 400 that says which; sent but not a `T` is a 422
/// by field, from a form or JSON (a 400 from the query, a 404 from the
/// route).
pub fn required<T: FromStr<Err: Display>>(cx: &Cx, name: &str) -> Result<T> {
    match find(cx, name) {
        Some((v, from)) => parse(name, &v, from),
        None if posts(cx) && is_json(cx) => {
            not_json(cx)?;
            Err(Error::invalid(name, "is required"))
        }
        None if posts(cx) => Err(Error::new(400, format!("missing form field `{name}`"))),
        None => Err(Error::new(400, format!("missing query parameter `{name}`"))),
    }
}

/// `name: Option<T>`: `None` when it is missing or blank.
pub fn optional<T: FromStr<Err: Display>>(cx: &Cx, name: &str) -> Result<Option<T>> {
    match find(cx, name) {
        Some((v, _)) if v.is_empty() => Ok(None),
        Some((v, from)) => parse(name, &v, from).map(Some),
        None => not_json(cx).map(|_| None),
    }
}

/// A JSON body that is not JSON: the 400 that says where, as `body: T`
/// gets, rather than every parameter read from it missing. Asked once a
/// parameter was looked for in it ([`json_values`]), so it costs a load.
fn not_json(cx: &Cx) -> Result<()> {
    if cx.json_failed() && !cx.body().trim_ascii().is_empty() {
        return crate::from_json::<Value>(cx.body()).map(|_| ());
    }
    Ok(())
}

/// `post: Post`, a struct with `#[derive(FromJson)]` (or `Rest`): a JSON
/// body read whole, or the form's fields by the struct's field names, with
/// its `#[validate]` rules. A blank field counts as missing; every problem
/// is listed by field, in one 422.
pub fn whole<T: FromJson>(cx: &Cx) -> Result<T> {
    if is_json(cx) {
        return crate::from_json(cx.body());
    }
    // A name's place in `members`: a scan while they are few, an index
    // once they are many (made then), so a form of thousands of fields is
    // not a scan of the ones before for each.
    let mut members: Vec<(String, Value)> = Vec::new();
    let mut index: HashMap<String, usize> = HashMap::new();
    for (k, v) in cx.form().iter() {
        let v = Value::String(v.into_owned());
        let found = if members.len() < 32 {
            members.iter().position(|(m, _)| *m == k)
        } else {
            if index.is_empty() {
                index.extend(members.iter().enumerate().map(|(i, (m, _))| (m.clone(), i)));
            }
            index.get(&*k).copied()
        };
        match found.map(|i| &mut members[i].1) {
            Some(Value::Array(items)) => items.push(v),
            Some(first) => *first = Value::Array(vec![std::mem::replace(first, Value::Null), v]),
            None => {
                if !index.is_empty() {
                    index.insert(k.to_string(), members.len());
                }
                members.push((k.into_owned(), v));
            }
        }
    }
    let mut problems = crate::json::Problems::form();
    match T::from_json(&Value::Object(members), &mut problems) {
        Some(v) if problems.is_empty() => Ok(v),
        _ => Err(problems.into_error()),
    }
}

/// A handler's input `r`, as its generated call reads each: `None` when
/// what was sent does not pass (a 422 by field), whose problems join the
/// others' in `problems`, so one answer lists them all. Any other error is
/// the answer.
pub fn read<T>(problems: &mut crate::json::Problems, r: Result<T>) -> Result<Option<T>> {
    match r {
        Ok(v) => Ok(Some(v)),
        Err(e) if e.status() == 422 && !e.fields().is_empty() => {
            for (field, problem) in e.fields() {
                problems.check(field, Some(problem.clone()));
            }
            Ok(None)
        }
        Err(e) => Err(e),
    }
}

/// The 422 of every input that did not pass (see [`read`]).
pub fn refused<T>(problems: crate::json::Problems) -> Result<T> {
    Err(problems.into_error())
}

/// `name: bool`: a checkbox, `true` when it was sent with any value but
/// `false`, `off` or `0`.
pub fn flag(cx: &Cx, name: &str) -> bool {
    find(cx, name).is_some_and(|(v, _)| on(&v))
}

/// Whether a checkbox's (or a switch's) value is on: anything but `false`,
/// `off` or `0`.
pub(crate) fn on(v: &str) -> bool {
    !matches!(v, "false" | "off" | "0")
}

/// `name: Vec<T>`: every value sent under the name, such as a group of
/// checkboxes; empty when there are none.
pub fn all<T: FromStr<Err: Display>>(cx: &Cx, name: &str) -> Result<Vec<T>> {
    let form: Vec<Cow<str>> = if !posts(cx) {
        Vec::new()
    } else if is_json(cx) {
        let sent: Vec<Cow<str>> = json_values(cx, name).collect();
        if sent.is_empty() {
            not_json(cx)?;
        }
        sent
    } else {
        cx.form().all(name).collect()
    };
    let (sent, from) = if form.is_empty() {
        (cx.query_all(name).collect(), From::Query)
    } else {
        (form, if is_json(cx) { From::Json } else { From::Form })
    };
    sent.iter().map(|v| parse(name, v, from)).collect()
}

/// `name: Image`: the file chosen in the form's field `name` (`None` for
/// none), or a 422 by the field when it is not an image.
pub fn image(cx: &Cx, name: &str) -> Result<Option<crate::Image>> {
    let Some(file) = cx.form().file(name) else {
        return Ok(None);
    };
    match crate::Image::new(file.bytes) {
        Some(image) => Ok(Some(image)),
        None => Err(Error::invalid(name, crate::image::NOT_AN_IMAGE)),
    }
}

/// An email address, checked as it is read: `#[action] fn join(email:
/// Email)` gets one, or the page shows again with "must be an email
/// address" by the input. Reads as the `&str` it holds.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Email(String);

impl FromStr for Email {
    type Err = String;

    fn from_str(s: &str) -> std::result::Result<Email, String> {
        let s = s.to_string();
        match crate::json::check::email(&s) {
            Some(problem) => Err(problem),
            None => Ok(Email(s)),
        }
    }
}

impl std::ops::Deref for Email {
    type Target = str;

    fn deref(&self) -> &str {
        &self.0
    }
}

impl AsRef<str> for Email {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl Display for Email {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::convert::From<Email> for String {
    fn from(e: Email) -> String {
        e.0
    }
}

impl crate::Json for Email {
    fn json(&self, out: &mut String) {
        self.0.json(out);
    }
}

impl FromJson for Email {
    fn from_json(v: &Value, p: &mut crate::json::Problems) -> Option<Email> {
        let s = String::from_json(v, p)?;
        s.parse().map_err(|e: String| p.add(e)).ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn email() {
        let e: Email = "ann@example.com".parse().unwrap();
        assert_eq!(
            (&*e, e.to_string()),
            ("ann@example.com", "ann@example.com".into())
        );
        assert!("ann".parse::<Email>().is_err());
        let post = cx(
            "POST /p HTTP/1.1\r\ncontent-type: application/x-www-form-urlencoded\r\ncontent-length: 9\r\n\r\nemail=ann",
            &[],
        );
        let bad = required::<Email>(&post, "email").unwrap_err();
        assert_eq!(bad.status(), 422);
        assert_eq!(bad.fields()[0].1, "must be an email address");
        let r: crate::Result<Email> = crate::from_json(br#""ann""#);
        assert_eq!(r.unwrap_err().status(), 422);
    }

    fn cx(raw: &str, params: &[(&'static str, &str)]) -> Cx {
        Cx::for_test(raw, params)
    }

    #[test]
    fn inputs_by_name() {
        let get = cx(
            "GET /p/7?q=tea&n=3&n=4&on=on&blank= HTTP/1.1\r\n\r\n",
            &[("id", "7")],
        );
        assert_eq!(required::<u32>(&get, "id").unwrap(), 7);
        assert_eq!(required::<String>(&get, "q").unwrap(), "tea");
        assert_eq!(optional::<String>(&get, "blank").unwrap(), None);
        assert_eq!(optional::<u8>(&get, "nope").unwrap(), None);
        assert_eq!(all::<u8>(&get, "n").unwrap(), [3, 4]);
        assert!(flag(&get, "on") && !flag(&get, "off"));
        let missing = required::<String>(&get, "text").unwrap_err();
        assert_eq!(
            (missing.status(), missing.message()),
            (400, "missing query parameter `text`")
        );
        assert_eq!(required::<u8>(&get, "q").unwrap_err().status(), 400);

        let bad = cx("GET /p/x HTTP/1.1\r\n\r\n", &[("id", "x")]);
        assert_eq!(required::<u32>(&bad, "id").unwrap_err().status(), 404);

        let post = cx(
            "POST /p?q=url HTTP/1.1\r\nContent-Type: application/x-www-form-urlencoded\r\n\r\ntext=hi+there&tag=a&tag=b&agree=false",
            &[],
        );
        assert_eq!(required::<String>(&post, "text").unwrap(), "hi there");
        assert_eq!(
            required::<String>(&post, "q").unwrap(),
            "url",
            "the query, when the form has none"
        );
        assert_eq!(all::<String>(&post, "tag").unwrap(), ["a", "b"]);
        assert!(!flag(&post, "agree"));
        assert_eq!(
            required::<String>(&post, "name").unwrap_err().message(),
            "missing form field `name`"
        );
    }

    #[test]
    fn json_bodies() {
        let post = cx(
            "POST /p HTTP/1.1\r\nContent-Type: application/json; charset=utf-8\r\n\r\n{\"text\":\"hi\",\"n\":3,\"on\":true,\"tags\":[\"a\",\"b\"],\"no\":null}",
            &[],
        );
        assert_eq!(required::<String>(&post, "text").unwrap(), "hi");
        assert_eq!(required::<u8>(&post, "n").unwrap(), 3);
        assert!(flag(&post, "on") && !flag(&post, "no"));
        assert_eq!(all::<String>(&post, "tags").unwrap(), ["a", "b"]);
        assert_eq!(optional::<u8>(&post, "no").unwrap(), None);
        let missing = required::<u8>(&post, "gone").unwrap_err();
        assert_eq!(
            (missing.status(), missing.fields()[0].1.as_str()),
            (422, "is required")
        );
        assert_eq!(required::<u8>(&post, "text").unwrap_err().status(), 422);
        let whole: crate::Value = body(&post).unwrap();
        assert_eq!(whole.get("n").and_then(|n| n.as_i64()), Some(3));

        // Not JSON: a 400 that says where, not each parameter missing.
        let broken = cx(
            "POST /p HTTP/1.1\r\nContent-Type: application/json\r\n\r\n{\"text\":",
            &[],
        );
        for e in [
            required::<String>(&broken, "text").unwrap_err(),
            optional::<String>(&broken, "text").unwrap_err(),
            all::<String>(&broken, "text").unwrap_err(),
        ] {
            assert_eq!(e.status(), 400);
            assert!(e.message().starts_with("Invalid JSON"), "{}", e.message());
        }
        let empty = cx(
            "POST /p HTTP/1.1\r\nContent-Type: application/json\r\n\r\n ",
            &[],
        );
        assert_eq!(optional::<String>(&empty, "text").unwrap(), None);

        let form = cx(
            "POST /p HTTP/1.1\r\nContent-Type: text/plain\r\n\r\n{}",
            &[],
        );
        assert_eq!(body::<crate::Value>(&form).unwrap_err().status(), 415);
        let bare = cx("POST /p HTTP/1.1\r\n\r\n[1]", &[]);
        assert_eq!(
            body::<Vec<u8>>(&bare).unwrap(),
            [1],
            "no Content-Type: tried as JSON"
        );
    }

    /// What `#[derive(FromJson)]` writes for a struct with named fields.
    #[derive(Debug, PartialEq)]
    struct Post {
        title: String,
        stars: u8,
        draft: bool,
        tags: Vec<String>,
        note: Option<String>,
    }

    impl FromJson for Post {
        fn from_json(v: &Value, p: &mut crate::json::Problems) -> Option<Post> {
            let m = p.object(v)?;
            let title: Option<String> = p.field(m, "title");
            if let Some(t) = &title {
                p.check("title", crate::json::check::max_len(t, 5));
            }
            let (stars, draft) = (p.field(m, "stars"), p.field(m, "draft"));
            let (tags, note) = (p.field(m, "tags"), p.field(m, "note"));
            Some(Post {
                title: title?,
                stars: stars?,
                draft: draft?,
                tags: tags?,
                note: note?,
            })
        }
    }

    fn form(body: &str) -> Cx {
        let raw = format!(
            "POST /p HTTP/1.1\r\nContent-Type: application/x-www-form-urlencoded\r\n\r\n{body}"
        );
        cx(&raw, &[])
    }

    #[test]
    fn a_struct_from_a_form() {
        let ok = whole::<Post>(&form("title=Hi&stars=3&draft=on&tags=a&note=")).unwrap();
        let want = Post {
            title: "Hi".into(),
            stars: 3,
            draft: true,
            tags: vec!["a".into()],
            note: None,
        };
        assert_eq!(ok, want, "text read by type; a blank field is left out");
        let two = whole::<Post>(&form("title=Hi&stars=3&tags=a&tags=b&tags=c")).unwrap();
        assert_eq!(
            (two.draft, two.tags),
            (false, vec!["a".into(), "b".into(), "c".into()])
        );
        // Many fields are read at once, not each looked for among the rest.
        let mut many: String = (0..200_000).map(|i| format!("f{i}=&")).collect();
        many.push_str("title=Hi&stars=1");
        assert_eq!(whole::<Post>(&form(&many)).unwrap().stars, 1);

        // Every problem, by field: blank is missing, text is not a number.
        let bad = whole::<Post>(&form("title=+&stars=x")).unwrap_err();
        assert_eq!(bad.status(), 422);
        assert_eq!(
            bad.fields(),
            [
                ("title".to_string(), "is required".to_string()),
                ("stars".into(), "expected a whole number".into()),
            ]
        );
        let long = whole::<Post>(&form("title=Longer&stars=300")).unwrap_err();
        assert_eq!(
            long.fields(),
            [
                (
                    "title".to_string(),
                    "must have at most 5 characters".to_string()
                ),
                ("stars".into(), "must be from 0 to 255".into()),
            ]
        );

        // A JSON body is JSON: a number as text is wrong there.
        let json = cx(
            "POST /p HTTP/1.1\r\nContent-Type: application/json\r\n\r\n{\"title\":\"Hi\",\"stars\":\"3\"}",
            &[],
        );
        let e = whole::<Post>(&json).unwrap_err();
        assert_eq!(e.fields()[0].1, "expected a number, found a string");
    }

    #[test]
    fn every_input_problem_is_kept() {
        let mut p = crate::json::Problems::default();
        let a: Option<u8> = read(&mut p, Ok(1)).unwrap();
        let b: Option<u8> = read(&mut p, Err(Error::invalid("b", "no"))).unwrap();
        let c: Option<u8> = read(&mut p, Err(Error::invalid("c", "nor").and("d", "this"))).unwrap();
        assert_eq!((a, b, c), (Some(1), None, None));
        let missing = read::<u8>(&mut p, Err(Error::new(400, "missing"))).unwrap_err();
        assert_eq!(missing.status(), 400, "any other error is the answer");
        let all = refused::<()>(p).unwrap_err();
        assert_eq!((all.status(), all.fields().len()), (422, 3));
    }
}
