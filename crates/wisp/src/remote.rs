//! `#[remote]` functions, which browser code calls as `await user(5)`:
//! their arguments by name, and a GET's answer made cacheable. The rest
//! (reading each argument, the call, its answer) is the generated shim's.

use crate::json::Value;
use crate::{Cx, Error, Method, Response, Result};

/// The arguments sent: the JSON object a POST's body is (none, no
/// arguments), or a GET's query, each value JSON (`?id=5&q=%22tea%22`) or,
/// when it is not, text (`?q=tea`).
pub fn args(cx: &Cx) -> Result<Value> {
    if cx.method != Method::Get && cx.method != Method::Head {
        if cx.body().is_empty() {
            return Ok(Value::Object(Vec::new()));
        }
        let v: Value = crate::input::body(cx)?;
        return match v {
            Value::Object(_) => Ok(v),
            _ => Err(Error::new(
                400,
                "Expected a JSON object of the arguments, by name",
            )),
        };
    }
    let members = cx
        .query_pairs()
        .map(|(k, v)| {
            let v = crate::json::parse(&v).unwrap_or_else(|_| Value::String(v.into_owned()));
            (k.into_owned(), v)
        })
        .collect();
    Ok(Value::Object(members))
}

/// The members of what [`args`] read.
pub fn members(v: &Value) -> &[(String, Value)] {
    match v {
        Value::Object(m) => m,
        _ => &[],
    }
}

/// A GET's answer: a 200 gets an `etag` of its body, so the browser keeps
/// it and asks again with `if-none-match`, answered 304 when it is the same.
pub fn get(res: Response) -> Response {
    if res.status != 200 || res.headers.iter().any(|(n, _)| n == "etag") {
        return res;
    }
    let tag = crate::rest::etag(&res.body);
    res.with_header("etag", tag)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_get_has_an_etag_once() {
        let r = get(Response::json("{\"a\":1}"));
        let tags: Vec<_> = r.headers.iter().filter(|(n, _)| n == "etag").collect();
        assert_eq!(tags.len(), 1);
        assert!(get(r).headers.iter().filter(|(n, _)| n == "etag").count() == 1);
        assert!(get(Response::empty(204)).headers.is_empty());
    }

    #[test]
    fn members_of_anything_else_are_none() {
        assert!(members(&Value::Null).is_empty());
        let v = Value::Object(vec![("id".into(), Value::Number("5".into()))]);
        assert_eq!(members(&v).len(), 1);
    }
}
