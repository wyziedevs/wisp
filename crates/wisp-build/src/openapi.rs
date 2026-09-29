//! An OpenAPI 3.1 description of the app's `+server.rs` endpoints, made at
//! build time from what the build already reads: each route's path, the
//! methods its file defines, their inputs by name and type, and what they
//! return. Types defined in the same `+server.rs` are described field by
//! field; any other type is named in a description and left open, rather
//! than guessed. `wisp` serves it at `/_wisp/openapi.json`, with a page to
//! try it at `/_wisp/docs`.

use crate::routes::{Route, Seg};
use crate::rust_scan::{self, FnItem, Returns, TypeItem};

/// One `+server.rs`: its route and what the file defines.
pub struct Endpoint<'a> {
    pub route: &'a Route,
    pub fns: &'a [FnItem],
    pub types: &'a [TypeItem],
}

/// The whole document, as JSON.
pub fn spec(title: &str, version: &str, endpoints: &[Endpoint]) -> String {
    let mut schemas: Vec<(String, String)> = Vec::new();
    let mut paths = Vec::new();
    for e in endpoints {
        let mut ops = Vec::new();
        for f in e.fns {
            ops.push(format!("{}:{}", q(&f.name), operation(e, f, &mut schemas)));
        }
        paths.push(format!("{}:{{{}}}", q(&path(e.route)), ops.join(",")));
    }
    schemas.push((
        "Error".into(),
        r#"{"type":"object","required":["status","error"],"properties":{"status":{"type":"integer"},"error":{"type":"string"},"errors":{"type":"object","additionalProperties":{"type":"string"},"description":"What is wrong, by field (a 422)"}}}"#.into(),
    ));
    let schemas: Vec<String> = schemas
        .iter()
        .map(|(n, s)| format!("{}:{s}", q(n)))
        .collect();
    format!(
        "{{\"openapi\":\"3.1.0\",\"info\":{{\"title\":{},\"version\":{}}},\"paths\":{{{}}},\"components\":{{\"schemas\":{{{}}}}}}}",
        q(title),
        q(version),
        paths.join(","),
        schemas.join(",")
    )
}

/// `/notes/[id]` as OpenAPI writes it: `/notes/{id}`.
fn path(r: &Route) -> String {
    if r.segs.is_empty() {
        return "/".into();
    }
    r.segs
        .iter()
        .map(|s| match s {
            Seg::Static(n) => format!("/{n}"),
            Seg::Param(n, _) | Seg::Optional(n, _) | Seg::Rest(n) => format!("/{{{n}}}"),
        })
        .collect()
}

fn operation(e: &Endpoint, f: &FnItem, schemas: &mut Vec<(String, String)>) -> String {
    let sends_body = matches!(f.name.as_str(), "post" | "put" | "patch");
    let mut params = Vec::new();
    let mut fields = Vec::new();
    let mut required = Vec::new();
    let mut body = None;
    for (name, ty) in f.inputs().unwrap_or_default() {
        let t: String = ty.chars().filter(|c| !c.is_whitespace()).collect();
        let optional = matches!(rust_scan::last_segment(&t), "Option" | "Vec") || t == "bool";
        let in_path = e.route.segs.iter().any(
            |s| matches!(s, Seg::Param(n, _) | Seg::Optional(n, _) | Seg::Rest(n) if n == name),
        );
        let text = |t: &str| rust_scan::last_segment(t) == "String" || t.contains("str");
        let schema = schema(&t, e.types, schemas);
        if name == "body" && !text(&t) && !inner(&t).is_some_and(text) {
            body = Some(schema);
        } else if in_path {
            params.push(format!(
                "{{\"name\":{},\"in\":\"path\",\"required\":true,\"schema\":{schema}}}",
                q(name)
            ));
        } else if sends_body {
            fields.push(format!("{}:{schema}", q(name)));
            if !optional {
                required.push(q(name));
            }
        } else {
            params.push(format!(
                "{{\"name\":{},\"in\":\"query\",\"required\":{},\"schema\":{schema}}}",
                q(name),
                !optional
            ));
        }
    }
    // Path parameters the function does not take are still in the path.
    for s in &e.route.segs {
        if let Seg::Param(n, _) | Seg::Optional(n, _) | Seg::Rest(n) = s
            && !params
                .iter()
                .any(|p| p.starts_with(&format!("{{\"name\":{},\"in\":\"path\"", q(n))))
        {
            params.push(format!(
                "{{\"name\":{},\"in\":\"path\",\"required\":true,\"schema\":{{\"type\":\"string\"}}}}",
                q(n)
            ));
        }
    }
    let mut out = format!(
        "{{\"operationId\":{}",
        q(&format!("{}{}", f.name, id(e.route)))
    );
    if !params.is_empty() {
        out.push_str(&format!(",\"parameters\":[{}]", params.join(",")));
    }
    let request = match body {
        Some(schema) => Some(format!("{{\"application/json\":{{\"schema\":{schema}}}}}")),
        None if !fields.is_empty() => {
            let schema = format!(
                "{{\"type\":\"object\",\"required\":[{}],\"properties\":{{{}}}}}",
                required.join(","),
                fields.join(",")
            );
            Some(format!(
                "{{\"application/json\":{{\"schema\":{schema}}},\"application/x-www-form-urlencoded\":{{\"schema\":{schema}}}}}"
            ))
        }
        None => None,
    };
    let validated = request.is_some();
    if let Some(content) = request {
        out.push_str(&format!(
            ",\"requestBody\":{{\"required\":true,\"content\":{content}}}"
        ));
    }
    let returns = if f.fallible {
        first_arg(f.returns.trim()).unwrap_or("")
    } else {
        f.returns.trim()
    };
    let mut responses = vec![match f.returns_kind() {
        Returns::Nothing => "\"204\":{\"description\":\"Done\"}".to_string(),
        Returns::Response => "\"200\":{\"description\":\"A response\"}".into(),
        Returns::MaybeResponse => {
            "\"200\":{\"description\":\"A response\"},\"404\":{\"description\":\"Not found\"}"
                .into()
        }
        Returns::Other => format!(
            "\"200\":{{\"description\":\"OK\",\"content\":{{\"application/json\":{{\"schema\":{}}}}}}}",
            schema(&returns.replace(char::is_whitespace, ""), e.types, schemas)
        ),
    }];
    let error = |status: &str, what: &str| {
        format!(
            "\"{status}\":{{\"description\":\"{what}\",\"content\":{{\"application/json\":{{\"schema\":{{\"$ref\":\"#/components/schemas/Error\"}}}}}}}}"
        )
    };
    if validated {
        responses.push(error("422", "The input did not pass"));
    }
    if f.fallible {
        responses.push(error("default", "An error"));
    }
    out.push_str(&format!(",\"responses\":{{{}}}}}", responses.join(",")));
    out
}

/// `/notes/[id]` as an identifier's tail: `_notes_id`.
fn id(r: &Route) -> String {
    path(r)
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect::<String>()
        .trim_end_matches('_')
        .replace("__", "_")
}

/// The JSON Schema of the Rust type `t` (no whitespace). Types defined in
/// the file are added to `schemas` and referred to.
fn schema(t: &str, types: &[TypeItem], schemas: &mut Vec<(String, String)>) -> String {
    // Whitespace is gone, so `&'static str` is `&'staticstr`.
    let t = match t.strip_prefix('&') {
        Some(r) => r.trim_start_matches("'static").trim_start_matches("mut"),
        None => t,
    };
    let last = rust_scan::last_segment(t);
    let arg = || inner(t).unwrap_or("");
    match last {
        "Option" | "Box" | "Arc" | "Rc" => schema(arg(), types, schemas),
        "Vec" | "VecDeque" | "BTreeSet" | "HashSet" => {
            format!(
                "{{\"type\":\"array\",\"items\":{}}}",
                schema(arg(), types, schemas)
            )
        }
        "BTreeMap" | "HashMap" => {
            let value = arg().split_once(',').map_or("", |(_, v)| v);
            format!(
                "{{\"type\":\"object\",\"additionalProperties\":{}}}",
                schema(value, types, schemas)
            )
        }
        "String" | "str" | "char" | "Cow" => "{\"type\":\"string\"}".into(),
        "bool" => "{\"type\":\"boolean\"}".into(),
        "u8" | "u16" | "u32" | "u64" | "u128" | "usize" => {
            "{\"type\":\"integer\",\"minimum\":0}".into()
        }
        "i8" | "i16" | "i32" | "i64" | "i128" | "isize" => "{\"type\":\"integer\"}".into(),
        "f32" | "f64" => "{\"type\":\"number\"}".into(),
        "Value" => "{}".into(),
        "()" => "{\"type\":\"null\"}".into(),
        _ if t.starts_with('[') => {
            let item = t[1..].split([';', ']']).next().unwrap_or("");
            format!(
                "{{\"type\":\"array\",\"items\":{}}}",
                schema(item, types, schemas)
            )
        }
        _ if t.starts_with('(') => "{\"type\":\"array\"}".into(),
        name => match types.iter().find(|ty| ty.name == name && !t.contains('<')) {
            Some(ty) => {
                if !schemas.iter().any(|(n, _)| n == name) {
                    // Placed first, so a type that contains itself refers back.
                    schemas.push((name.to_string(), "{}".into()));
                    let at = schemas.len() - 1;
                    schemas[at].1 = object(ty, types, schemas);
                }
                format!("{{\"$ref\":\"#/components/schemas/{name}\"}}")
            }
            None => format!("{{\"description\":{}}}", q(t)),
        },
    }
}

/// A struct defined in the file, field by field. One with no named fields
/// (an enum, a tuple struct) is only named.
fn object(ty: &TypeItem, types: &[TypeItem], schemas: &mut Vec<(String, String)>) -> String {
    if ty.fields.is_empty() {
        return format!("{{\"title\":{}}}", q(&ty.name));
    }
    let mut props = Vec::new();
    let mut required = Vec::new();
    for (name, fty) in &ty.fields {
        let t: String = fty.chars().filter(|c| !c.is_whitespace()).collect();
        let name = name.strip_prefix("r#").unwrap_or(name);
        props.push(format!("{}:{}", q(name), schema(&t, types, schemas)));
        if rust_scan::last_segment(&t) != "Option" {
            required.push(q(name));
        }
    }
    format!(
        "{{\"type\":\"object\",\"required\":[{}],\"properties\":{{{}}}}}",
        required.join(","),
        props.join(",")
    )
}

/// What is inside the first `<...>` of `t`.
fn inner(t: &str) -> Option<&str> {
    let open = t.find('<')?;
    t[open + 1..].strip_suffix('>')
}

/// `Result<Vec<Note>, E>` → `Vec<Note>`.
fn first_arg(t: &str) -> Option<&str> {
    let inner = inner(t)?;
    let mut depth = 0;
    for (i, c) in inner.char_indices() {
        match c {
            '<' | '(' | '[' => depth += 1,
            '>' | ')' | ']' => depth -= 1,
            ',' if depth == 0 => return Some(inner[..i].trim()),
            _ => {}
        }
    }
    Some(inner.trim())
}

/// A JSON string.
fn q(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn describes_endpoints() {
        let items = rust_scan::scan(
            "struct Note { id: u64, title: String, tags: Vec<String>, due: Option<String>, author: crate::User }\n\
             struct New { title: String }\n\
             fn get(q: Option<String>, limit: u32) -> Vec<Note> { todo!() }\n\
             async fn post(body: New) -> Result<Note> { todo!() }\n\
             fn put(id: u64, title: String, done: bool) -> Option<Response> { None }\n\
             fn delete(cx: &mut Cx) {}",
        )
        .unwrap();
        let route = Route {
            dir: PathBuf::new(),
            segs: vec![Seg::Static("api".into()), Seg::Param("id".into(), None)],
            page: false,
            page_rs: false,
            page_js: false,
            server: true,
            layouts: Vec::new(),
            error: None,
        };
        let json = spec(
            "app",
            "0.1.0",
            &[Endpoint {
                route: &route,
                fns: &items.fns,
                types: &items.types,
            }],
        );
        for want in [
            "\"openapi\":\"3.1.0\",\"info\":{\"title\":\"app\",\"version\":\"0.1.0\"}",
            "\"paths\":{\"/api/{id}\":{\"get\":{\"operationId\":\"get_api_id\"",
            "{\"name\":\"q\",\"in\":\"query\",\"required\":false,\"schema\":{\"type\":\"string\"}}",
            "{\"name\":\"limit\",\"in\":\"query\",\"required\":true,\"schema\":{\"type\":\"integer\",\"minimum\":0}}",
            "{\"name\":\"id\",\"in\":\"path\",\"required\":true,\"schema\":{\"type\":\"string\"}}",
            "\"200\":{\"description\":\"OK\",\"content\":{\"application/json\":{\"schema\":{\"type\":\"array\",\"items\":{\"$ref\":\"#/components/schemas/Note\"}}}}}",
            "\"requestBody\":{\"required\":true,\"content\":{\"application/json\":{\"schema\":{\"$ref\":\"#/components/schemas/New\"}}}}",
            "\"422\":{\"description\":\"The input did not pass\"",
            "{\"name\":\"id\",\"in\":\"path\",\"required\":true,\"schema\":{\"type\":\"integer\",\"minimum\":0}}",
            "\"required\":[\"title\"],\"properties\":{\"title\":{\"type\":\"string\"},\"done\":{\"type\":\"boolean\"}}",
            "\"404\":{\"description\":\"Not found\"}",
            "\"delete\":{\"operationId\":\"delete_api_id\",\"parameters\":[{\"name\":\"id\"",
            "\"204\":{\"description\":\"Done\"}",
            "\"Note\":{\"type\":\"object\",\"required\":[\"id\",\"title\",\"tags\",\"author\"]",
            "\"due\":{\"type\":\"string\"},\"author\":{\"description\":\"crate::User\"}",
            "\"Error\":{\"type\":\"object\"",
        ] {
            assert!(json.contains(want), "{want}\n{json}");
        }
    }
}
