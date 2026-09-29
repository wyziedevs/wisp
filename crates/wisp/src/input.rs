//! A handler's parameters other than `cx`: `fn load(slug: String, page:
//! Option<u32>)`, `#[action] fn add(text: String)`. The generated call reads
//! each by its name: a route parameter first, then the form a POST, PUT or
//! PATCH sends, then the URL's query. The type says how: `Option` when it
//! may be left out (or blank), `bool` for a checkbox, `Vec` for every value
//! of a repeated field, anything else `FromStr` when it must be there.

use crate::{Cx, Error, Method, Result};
use std::borrow::Cow;
use std::fmt::Display;
use std::str::FromStr;

/// Where a value was found, for the error that says it is wrong.
#[derive(Clone, Copy)]
enum From {
    Param,
    Form,
    Query,
}

fn posts(cx: &Cx) -> bool {
    matches!(cx.method, Method::Post | Method::Put | Method::Patch)
}

fn find<'a>(cx: &'a Cx, name: &str) -> Option<(Cow<'a, str>, From)> {
    if let Some(v) = cx.route_param(name) {
        return Some((Cow::Borrowed(v), From::Param));
    }
    if posts(cx)
        && let Some(v) = cx.form().get(name)
    {
        return Some((v, From::Form));
    }
    cx.query(name).map(|v| (v, From::Query))
}

fn parse<T: FromStr<Err: Display>>(name: &str, v: &str, from: From) -> Result<T> {
    v.parse().map_err(|e| match from {
        // A path whose segment is not one of these: there is no such page.
        From::Param => Error::new(404, "Not Found"),
        From::Form => Error::new(400, format!("form field `{name}`: {e}")),
        From::Query => Error::new(400, format!("query parameter `{name}`: {e}")),
    })
}

/// `name: T`: missing, or not a `T`, is a 400 that says which (a route
/// parameter that is not a `T` is a 404).
pub fn required<T: FromStr<Err: Display>>(cx: &Cx, name: &str) -> Result<T> {
    match find(cx, name) {
        Some((v, from)) => parse(name, &v, from),
        None if posts(cx) => Err(Error::new(400, format!("missing form field `{name}`"))),
        None => Err(Error::new(400, format!("missing query parameter `{name}`"))),
    }
}

/// `name: Option<T>`: `None` when it is missing or blank.
pub fn optional<T: FromStr<Err: Display>>(cx: &Cx, name: &str) -> Result<Option<T>> {
    match find(cx, name) {
        Some((v, _)) if v.is_empty() => Ok(None),
        Some((v, from)) => parse(name, &v, from).map(Some),
        None => Ok(None),
    }
}

/// `name: bool`: a checkbox, `true` when it was sent with any value but
/// `false`, `off` or `0`.
pub fn flag(cx: &Cx, name: &str) -> bool {
    find(cx, name).is_some_and(|(v, _)| !matches!(&*v, "false" | "off" | "0"))
}

/// `name: Vec<T>`: every value sent under the name, such as a group of
/// checkboxes; empty when there are none.
pub fn all<T: FromStr<Err: Display>>(cx: &Cx, name: &str) -> Result<Vec<T>> {
    let form: Vec<Cow<str>> = if posts(cx) {
        cx.form().all(name).collect()
    } else {
        Vec::new()
    };
    let (sent, from) = if form.is_empty() {
        (cx.query_all(name).collect(), From::Query)
    } else {
        (form, From::Form)
    };
    sent.iter().map(|v| parse(name, v, from)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
