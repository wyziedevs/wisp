//! AWS Lambda (`provided.al2023`, `wisp build --target lambda`): with
//! `AWS_LAMBDA_RUNTIME_API` set, [`crate::run`] takes requests from Lambda's
//! runtime API instead of a socket, one at a time, as Lambda hands them out.
//! Function URLs and API Gateway HTTP APIs (payload 2.0), and REST APIs and
//! ALBs (1.0) are read; the reply goes back in the shape its event came in.
//!
//! The runtime API is plain HTTP on the instance, so this is a few lines of
//! `std` and `httparse`: no `lambda_runtime`, no `hyper`.

use crate::json::{self, Value};
use crate::{App, Body, Json, Reply, Request};
use std::io::{Read, Write};
use std::net::{IpAddr, SocketAddr, TcpStream};

const NEXT: &str = "/2018-06-01/runtime/invocation/next";

/// Runs `init`, then answers events until Lambda stops the instance. A
/// runtime API that cannot be reached ends the process, and Lambda starts
/// another.
pub(crate) fn run<A: App>(api: &str) {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap_or_else(|e| crate::fail(&format!("could not start tokio: {e}")));
    if let Err(e) = rt.block_on(crate::prepare::<A>()) {
        let why = error_json(&e.to_string());
        let _ = call(
            api,
            "POST",
            "/2018-06-01/runtime/init/error",
            why.as_bytes(),
        );
        crate::fail(&e.to_string());
    }
    loop {
        let (id, event) = call(api, "GET", NEXT, b"")
            .unwrap_or_else(|e| crate::fail(&format!("Lambda's runtime API at {api}: {e}")));
        let (path, answer) = match request(&event) {
            Some((req, shape)) => {
                let reply = rt.block_on(async {
                    let mut reply = crate::handle::<A>(req).await;
                    if let Body::Stream(rx) = &mut reply.body {
                        let mut all = Vec::new();
                        while let Some(chunk) = rx.recv().await {
                            all.extend_from_slice(&chunk);
                        }
                        reply.body = Body::Bytes(all);
                    }
                    reply
                });
                ("response", reply_json(&reply, shape))
            }
            None => ("error", error_json("the event is not an HTTP request")),
        };
        let path = format!("/2018-06-01/runtime/invocation/{id}/{path}");
        if let Err(e) = call(api, "POST", &path, answer.as_bytes()) {
            crate::http::log(format_args!("wisp: could not answer Lambda: {e}"));
        }
    }
}

/// One call to the runtime API: the request id it names, and the body.
/// HTTP/1.0, so the body is never chunked and ends with the connection.
fn call(api: &str, method: &str, path: &str, body: &[u8]) -> std::io::Result<(String, Vec<u8>)> {
    let mut s = TcpStream::connect(api)?;
    let head = format!(
        "{method} {path} HTTP/1.0\r\nhost: {api}\r\ncontent-length: {}\r\n\r\n",
        body.len()
    );
    s.write_all(head.as_bytes())?;
    s.write_all(body)?;
    let mut all = Vec::new();
    s.read_to_end(&mut all)?;
    let mut headers = [httparse::EMPTY_HEADER; 32];
    let mut res = httparse::Response::new(&mut headers);
    let bad = |why: String| std::io::Error::other(why);
    let Ok(httparse::Status::Complete(n)) = res.parse(&all) else {
        return Err(bad("an answer that is not HTTP".into()));
    };
    let code = res.code.unwrap_or(0);
    if !(200..300).contains(&code) {
        let text = String::from_utf8_lossy(&all[n..]);
        return Err(bad(format!("{method} {path} answered {code}: {text}")));
    }
    let id = res
        .headers
        .iter()
        .find(|h| h.name.eq_ignore_ascii_case("lambda-runtime-aws-request-id"))
        .map_or(String::new(), |h| {
            String::from_utf8_lossy(h.value).into_owned()
        });
    Ok((id, all[n..].to_vec()))
}

/// The shape an event came in, which its reply goes back in.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Shape {
    /// Payload 2.0: `cookies`, one value per header.
    V2,
    /// Payload 1.0 with `multiValueHeaders` (REST APIs, ALBs that ask for them).
    Multi,
    /// Payload 1.0 with `headers` only (an ALB without multi-value headers).
    Single,
}

/// The request an HTTP event holds, and the shape of its payload.
fn request(event: &[u8]) -> Option<(Request, Shape)> {
    let v = json::parse(std::str::from_utf8(event).ok()?).ok()?;
    let ctx = v.get("requestContext");
    let http = ctx.and_then(|c| c.get("http"));
    let v2 = http.is_some();
    let mut single = false;
    let method = match http {
        Some(h) => h.get("method"),
        None => v.get("httpMethod"),
    }?;
    let mut req = Request::new(method.as_str()?, "");
    let text = |key: &str| v.get(key).and_then(Value::as_str).unwrap_or("");
    if v2 {
        req.target = text("rawPath").to_string();
        if !text("rawQueryString").is_empty() {
            req.target = format!("{}?{}", req.target, text("rawQueryString"));
        }
        each(v.get("headers"), |n, value| req.header(n, value));
        let cookies: Vec<&str> = list(v.get("cookies")).filter_map(Value::as_str).collect();
        if !cookies.is_empty() {
            req.header("cookie", &cookies.join("; "));
        }
    } else {
        // An ALB passes the query as the client sent it; API Gateway decodes it.
        let raw = ctx.and_then(|c| c.get("elb")).is_some();
        req.target = text("path").to_string();
        let mut sep = '?';
        let multi = v.get("multiValueQueryStringParameters");
        let query = if multi.is_some_and(|m| !m.is_null()) {
            multi
        } else {
            v.get("queryStringParameters")
        };
        each(query, |n, value| {
            req.target.push(sep);
            sep = '&';
            for (i, part) in [n, value].into_iter().enumerate() {
                if i == 1 {
                    req.target.push('=');
                }
                if raw {
                    req.target.push_str(part);
                } else {
                    encode(&mut req.target, part);
                }
            }
        });
        let multi = v.get("multiValueHeaders");
        let headers = if multi.is_some_and(|m| !m.is_null()) {
            multi
        } else {
            v.get("headers")
        };
        each(headers, |n, value| req.header(n, value));
        // An ALB with multi-value headers off ignores `multiValueHeaders`
        // in the reply, and so loses every header.
        single = !multi.is_some_and(|m| !m.is_null());
    }
    let ip = match http {
        Some(h) => h.get("sourceIp"),
        None => ctx
            .and_then(|c| c.get("identity"))
            .and_then(|i| i.get("sourceIp")),
    };
    if let Some(ip) = ip
        .and_then(Value::as_str)
        .and_then(|ip| ip.parse::<IpAddr>().ok())
    {
        req.peer = SocketAddr::new(ip, 0);
    }
    let body = text("body");
    req.body = if v.get("isBase64Encoded").and_then(Value::as_bool) == Some(true) {
        let mut out = vec![0; body.len() / 4 * 3 + 3];
        let n = wisp_shared::base64::decode(body, &mut out)?;
        out.truncate(n);
        out
    } else {
        body.as_bytes().to_vec()
    };
    let shape = match (v2, single) {
        (true, _) => Shape::V2,
        (false, true) => Shape::Single,
        (false, false) => Shape::Multi,
    };
    Some((req, shape))
}

fn list(v: Option<&Value>) -> impl Iterator<Item = &Value> {
    v.and_then(Value::as_array).unwrap_or(&[]).iter()
}

/// Each `name: value` of an object whose values are strings or lists of them.
fn each<'v>(v: Option<&'v Value>, mut f: impl FnMut(&'v str, &'v str)) {
    let Some(Value::Object(members)) = v else {
        return;
    };
    for (name, value) in members {
        match value {
            Value::String(s) => f(name, s),
            Value::Array(items) => items
                .iter()
                .filter_map(Value::as_str)
                .for_each(|s| f(name, s)),
            _ => {}
        }
    }
}

/// Percent-encodes all but the unreserved characters, for a query part
/// API Gateway decoded.
fn encode(out: &mut String, s: &str) {
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
}

/// The reply as Lambda takes it: payload 2.0 has `cookies` and one value
/// per header, 1.0 has `multiValueHeaders`. A body that is not UTF-8 goes
/// as base64.
fn reply_json(reply: &Reply, shape: Shape) -> String {
    let v2 = shape == Shape::V2;
    let mut out = format!("{{\"statusCode\":{},", reply.status);
    let cookie = |n: &str| v2 && n.eq_ignore_ascii_case("set-cookie");
    out.push_str(if shape == Shape::Multi {
        "\"multiValueHeaders\":{"
    } else {
        "\"headers\":{"
    });
    let mut first = true;
    for (i, (name, _)) in reply.headers.iter().enumerate() {
        let earlier = reply.headers[..i]
            .iter()
            .any(|(n, _)| n.eq_ignore_ascii_case(name));
        if earlier || cookie(name) {
            continue;
        }
        if !std::mem::take(&mut first) {
            out.push(',');
        }
        let values: Vec<&str> = reply
            .headers
            .iter()
            .filter(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| &**v)
            .collect();
        if shape == Shape::Single && name.eq_ignore_ascii_case("set-cookie") {
            // One value per key: the cookies go under keys that differ in case.
            for (k, value) in values.iter().enumerate() {
                if k > 0 {
                    out.push(',');
                }
                alternate("set-cookie", k).json(&mut out);
                out.push(':');
                value.json(&mut out);
            }
        } else {
            name.to_ascii_lowercase().json(&mut out);
            out.push(':');
            if shape == Shape::Multi {
                values.json(&mut out);
            } else {
                values.join(", ").json(&mut out);
            }
        }
    }
    out.push('}');
    if v2 {
        let cookies: Vec<&str> = reply
            .headers
            .iter()
            .filter(|(n, _)| cookie(n))
            .map(|(_, v)| &**v)
            .collect();
        out.push_str(",\"cookies\":");
        cookies.json(&mut out);
    }
    out.push_str(",\"body\":");
    match std::str::from_utf8(reply.bytes()) {
        Ok(text) => {
            text.json(&mut out);
            out.push_str(",\"isBase64Encoded\":false}");
        }
        Err(_) => {
            out.push('"');
            wisp_shared::base64::encode(&mut out, reply.bytes(), false);
            out.push_str("\",\"isBase64Encoded\":true}");
        }
    }
    out
}

/// `name` with the case of its letters set by the bits of `k`: a different
/// key for each `k`, which a client reads as the same header.
fn alternate(name: &str, k: usize) -> String {
    let mut letter = 0;
    name.chars()
        .map(|c| {
            if !c.is_ascii_alphabetic() {
                return c;
            }
            let up = letter < usize::BITS && k >> letter & 1 == 1;
            letter += 1;
            if up { c.to_ascii_uppercase() } else { c }
        })
        .collect()
}

fn error_json(message: &str) -> String {
    let mut out = String::from("{\"errorType\":\"Wisp\",\"errorMessage\":");
    message.json(&mut out);
    out.push('}');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_function_url_events() {
        let event = br#"{"version":"2.0","rawPath":"/a b","rawQueryString":"x=1&y=%20","cookies":["s=1","t=2"],
            "headers":{"content-type":"text/plain","x-many":"a,b"},
            "requestContext":{"http":{"method":"POST","sourceIp":"203.0.113.9"}},
            "body":"aGk=","isBase64Encoded":true}"#;
        let (req, shape) = request(event).unwrap();
        assert_eq!(shape, Shape::V2);
        assert_eq!(
            (req.method.as_str(), req.target.as_str()),
            ("POST", "/a b?x=1&y=%20")
        );
        assert!(req.headers.contains(&("cookie".into(), "s=1; t=2".into())));
        assert!(req.headers.contains(&("x-many".into(), "a,b".into())));
        assert_eq!(req.body, b"hi");
        assert_eq!(req.peer.ip().to_string(), "203.0.113.9");
    }

    #[test]
    fn reads_rest_api_and_alb_events() {
        let event = br#"{"httpMethod":"GET","path":"/p","multiValueQueryStringParameters":{"q":["a b","c"]},
            "multiValueHeaders":{"accept":["text/html"]},"requestContext":{"identity":{"sourceIp":"::1"}},"body":null}"#;
        let (req, shape) = request(event).unwrap();
        assert_eq!(shape, Shape::Multi);
        assert_eq!(req.target, "/p?q=a%20b&q=c");
        assert_eq!(
            req.headers,
            [("accept".to_string(), "text/html".to_string())]
        );
        let alb = br#"{"httpMethod":"GET","path":"/","queryStringParameters":{"q":"a%20b"},"requestContext":{"elb":{}}}"#;
        assert_eq!(request(alb).unwrap().0.target, "/?q=a%20b");
        assert!(request(b"{\"source\":\"aws.events\"}").is_none());
    }

    #[test]
    fn an_alb_without_multi_value_headers_is_answered_with_headers() {
        let event = br#"{"httpMethod":"GET","path":"/","headers":{"accept":"*/*"},"requestContext":{"elb":{}}}"#;
        let (_, shape) = request(event).unwrap();
        assert_eq!(shape, Shape::Single);
        let reply = Reply {
            status: 200,
            headers: vec![
                ("Content-Type".into(), "text/plain".into()),
                ("set-cookie".into(), "a=1".into()),
                ("set-cookie".into(), "b=2".into()),
                ("set-cookie".into(), "c=3".into()),
            ],
            body: Body::Bytes(b"hi".to_vec()),
        };
        let out = reply_json(&reply, shape);
        let v = json::parse(&out).unwrap();
        let headers = v.get("headers").expect("headers, not multiValueHeaders");
        assert!(v.get("multiValueHeaders").is_none());
        assert_eq!(
            headers.get("content-type").and_then(Value::as_str),
            Some("text/plain")
        );
        let cookies: Vec<&str> = match headers {
            Value::Object(m) => m
                .iter()
                .filter(|(n, _)| n.eq_ignore_ascii_case("set-cookie"))
                .filter_map(|(_, v)| v.as_str())
                .collect(),
            _ => Vec::new(),
        };
        assert_eq!(cookies, ["a=1", "b=2", "c=3"], "{out}");
    }

    #[test]
    fn writes_replies_per_payload() {
        let mut reply = Reply {
            status: 200,
            headers: vec![
                ("Content-Type".into(), "text/plain".into()),
                ("set-cookie".into(), "a=1".into()),
                ("set-cookie".into(), "b=2".into()),
            ],
            body: Body::Bytes(b"hi \"there\"".to_vec()),
        };
        assert_eq!(
            reply_json(&reply, Shape::V2),
            r#"{"statusCode":200,"headers":{"content-type":"text/plain"},"cookies":["a=1","b=2"],"body":"hi \"there\"","isBase64Encoded":false}"#
        );
        reply.body = Body::Bytes(vec![0xff, 0]);
        assert_eq!(
            reply_json(&reply, Shape::Multi),
            r#"{"statusCode":200,"multiValueHeaders":{"content-type":["text/plain"],"set-cookie":["a=1","b=2"]},"body":"/wA=","isBase64Encoded":true}"#
        );
    }
}
