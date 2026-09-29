//! An OpenAPI 3.1 description of the app's `+server.rs` endpoints, made at
//! build time from what the build already reads: each route's path, the
//! methods its file defines, their inputs by name and type, and what they
//! return. Types defined in the same `+server.rs` are described field by
//! field; any other type is named in a description and left open, rather
//! than guessed. `wisp` serves it at `/_wisp/openapi.json`, with a page to
//! try it at `/_wisp/docs`. The same data makes a TypeScript client, one
//! module with a typed method per endpoint.

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
        r#"{"type":"object","required":["status","code","error"],"properties":{"status":{"type":"integer"},"code":{"type":"string"},"error":{"type":"string"},"errors":{"type":"object","additionalProperties":{"type":"string"},"description":"What is wrong, by field (a 422)"}}}"#.into(),
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
    let mut params = Vec::new();
    let mut fields = Vec::new();
    let mut required = Vec::new();
    let mut body = None;
    for (name, ty) in f.inputs().unwrap_or_default() {
        let t: String = ty.chars().filter(|c| !c.is_whitespace()).collect();
        let optional = optional(&t);
        let schema = schema(&t, e.types, schemas);
        match place(e, f, name, &t) {
            Place::Body => body = Some(schema),
            Place::Path => params.push(format!(
                "{{\"name\":{},\"in\":\"path\",\"required\":true,\"schema\":{schema}}}",
                q(name)
            )),
            Place::Field => {
                fields.push(format!("{}:{schema}", q(name)));
                if !optional {
                    required.push(q(name));
                }
            }
            Place::Query => params.push(format!(
                "{{\"name\":{},\"in\":\"query\",\"required\":{},\"schema\":{schema}}}",
                q(name),
                !optional
            )),
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
    let mut out = format!("{{\"operationId\":{}", q(&operation_id(e, f)));
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
        Returns::Other => match f.optional_value() {
            Some("()") => {
                "\"204\":{\"description\":\"Done\"},\"404\":{\"description\":\"Not found\"}".into()
            }
            Some(t) => format!(
                "\"200\":{{\"description\":\"OK\",\"content\":{{\"application/json\":{{\"schema\":{}}}}}}},\"404\":{{\"description\":\"Not found\"}}",
                schema(&t.replace(char::is_whitespace, ""), e.types, schemas)
            ),
            None => format!(
                "\"200\":{{\"description\":\"OK\",\"content\":{{\"application/json\":{{\"schema\":{}}}}}}}",
                schema(&returns.replace(char::is_whitespace, ""), e.types, schemas)
            ),
        },
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

/// Where an input of a `+server.rs` function travels.
enum Place {
    /// A segment of the path.
    Path,
    /// The whole JSON body: an input named `body` that is not text.
    Body,
    /// One field of a post, put or patch body.
    Field,
    /// A query parameter.
    Query,
}

fn place(e: &Endpoint, f: &FnItem, name: &str, t: &str) -> Place {
    let text = |t: &str| rust_scan::last_segment(t) == "String" || t.contains("str");
    let in_path =
        e.route.segs.iter().any(
            |s| matches!(s, Seg::Param(n, _) | Seg::Optional(n, _) | Seg::Rest(n) if n == name),
        );
    if name == "body" && !text(t) && !inner(t).is_some_and(text) {
        Place::Body
    } else if in_path {
        Place::Path
    } else if matches!(f.name.as_str(), "post" | "put" | "patch") {
        Place::Field
    } else {
        Place::Query
    }
}

/// Whether an input (`t` has no whitespace) may be left out.
fn optional(t: &str) -> bool {
    matches!(rust_scan::last_segment(t), "Option" | "Vec") || t == "bool"
}

/// `get_api_notes_id`: the method and the route.
fn operation_id(e: &Endpoint, f: &FnItem) -> String {
    format!("{}{}", f.name, id(e.route))
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

/// `t` without a leading reference. Whitespace is gone, so `&'static str`
/// is `&'staticstr`.
fn unref(t: &str) -> &str {
    match t.strip_prefix('&') {
        Some(r) => r.trim_start_matches("'static").trim_start_matches("mut"),
        None => t,
    }
}

/// The JSON Schema of the Rust type `t` (no whitespace). Types defined in
/// the file are added to `schemas` and referred to.
fn schema(t: &str, types: &[TypeItem], schemas: &mut Vec<(String, String)>) -> String {
    let t = unref(t);
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
        // Wisp's row: the fields of `T`, `id` first.
        "Row" if inner(t).is_some() => format!(
            "{{\"allOf\":[{{\"type\":\"object\",\"required\":[\"id\"],\"properties\":{{\"id\":{{\"type\":\"integer\",\"minimum\":0}}}}}},{}]}}",
            schema(arg(), types, schemas)
        ),
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

/// `schema` with the limits of a field's `#[validate(…)]`: `min`, `max`,
/// `min_len`, `max_len`, `len` (a range) and `email`. A limit that is not a
/// plain number (a constant, say) is left out, not guessed.
fn constrain(mut schema: String, rules: &str, ty: &str) -> String {
    let (min_len, max_len) = if ty.contains("Vec<") {
        ("minItems", "maxItems")
    } else {
        ("minLength", "maxLength")
    };
    let mut add = |key: &str, n: &str| {
        if n.parse::<f64>().is_err() {
            return;
        }
        // Unsigned integers already say `minimum`: the rule replaces it.
        if key == "minimum" {
            schema = schema.replace(",\"minimum\":0", "");
        }
        schema.pop();
        schema.push_str(&format!(",\"{key}\":{n}}}"));
    };
    let mut email = false;
    for rule in rules.split(',').map(str::trim) {
        let (key, x) = rule
            .split_once('=')
            .map_or((rule, ""), |(k, x)| (k.trim(), x.trim()));
        match key {
            "min" => add("minimum", x),
            "max" => add("maximum", x),
            "min_len" => add(min_len, x),
            "max_len" => add(max_len, x),
            "len" => {
                let x: String = x.chars().filter(|c| !c.is_whitespace()).collect();
                let Some((lo, hi)) = x.split_once("..") else {
                    continue;
                };
                add(min_len, lo);
                match hi.strip_prefix('=') {
                    Some(hi) => add(max_len, hi),
                    // `..10` stops short of 10.
                    None => add(
                        max_len,
                        &hi.parse::<u64>()
                            .map_or(String::new(), |h| h.saturating_sub(1).to_string()),
                    ),
                }
            }
            "email" => email = true,
            _ => {}
        }
    }
    if email {
        schema.pop();
        schema.push_str(",\"format\":\"email\"}");
    }
    schema
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
        let mut schema = schema(&t, types, schemas);
        if let Some((_, rules)) = ty.rules.iter().find(|(f, _)| f == name) {
            schema = constrain(schema, rules, &t);
        }
        props.push(format!("{}:{schema}", q(name)));
        if !may_leave_out(ty, name, &t) {
            required.push(q(name));
        }
    }
    format!(
        "{{\"type\":\"object\",\"required\":[{}],\"properties\":{{{}}}}}",
        required.join(","),
        props.join(",")
    )
}

/// A TypeScript client for the same endpoints: one self-contained module
/// (no imports, needs only a global `fetch`) with an interface per struct
/// the endpoints use and one async method per operation, named as its
/// `operationId` in camel case. Path parameters come first, then the body,
/// then a `query` object.
pub fn typescript(endpoints: &[Endpoint]) -> String {
    let mut decls: Vec<(String, String)> = Vec::new();
    let mut methods = String::new();
    for e in endpoints {
        for f in e.fns {
            methods.push_str(&method(e, f, &mut decls));
        }
    }
    let mut out =
        String::from("// Generated by Wisp from the app's +server.rs files. Do not edit.\n\n");
    for (_, decl) in &decls {
        out.push_str(decl);
        out.push('\n');
    }
    out.push_str(TS_RUNTIME);
    out.push_str(&methods);
    out.push_str("  };\n}\n");
    out
}

/// What every generated client shares: the error, the options, and the one
/// function that talks to the server. The operations follow it.
const TS_RUNTIME: &str = r#"export interface ApiError {
  status: number;
  code: string;
  error: string;
  errors?: Record<string, string>;
}

export class WispError extends Error {
  status: number;
  code: string;
  errors?: Record<string, string>;
  constructor(status: number, body: Partial<ApiError>, statusText: string) {
    super(body.error ?? statusText);
    this.name = "WispError";
    this.status = status;
    this.code = body.code ?? "";
    this.errors = body.errors;
  }
}

export interface Options {
  base?: string;
  token?: string;
  headers?: Record<string, string>;
  fetch?: typeof fetch;
}

export function client(options: Options = {}) {
  async function call<T>(method: string, path: string, query?: object, body?: unknown): Promise<T> {
    const params = new URLSearchParams();
    for (const [key, value] of Object.entries(query ?? {})) {
      for (const v of Array.isArray(value) ? value : [value]) {
        if (v !== undefined && v !== null) params.append(key, String(v));
      }
    }
    const headers: Record<string, string> = {};
    if (body !== undefined) headers["content-type"] = "application/json";
    if (options.token) headers.authorization = `Bearer ${options.token}`;
    Object.assign(headers, options.headers);
    const search = params.toString();
    const url = (options.base ?? "") + path + (search ? "?" + search : "");
    const res = await (options.fetch ?? fetch)(url, {
      method,
      headers,
      body: body === undefined ? undefined : JSON.stringify(body),
    });
    const text = res.status === 204 ? "" : await res.text();
    let data: unknown;
    try {
      data = text ? JSON.parse(text) : undefined;
    } catch {
      data = text;
    }
    if (!res.ok) {
      const fail = (typeof data === "object" && data ? data : {}) as Partial<ApiError>;
      throw new WispError(res.status, fail, res.statusText);
    }
    return data as T;
  }
  return {
"#;

/// Words a path parameter cannot be called in the client, or it would hide
/// something the method body uses.
const TS_TAKEN: [&str; 24] = [
    "body", "call", "query", "options", "delete", "default", "new", "class", "function", "var",
    "let", "const", "if", "else", "for", "while", "in", "of", "this", "null", "true", "false",
    "void", "typeof",
];

/// One entry of the object `client` returns.
fn method(e: &Endpoint, f: &FnItem, decls: &mut Vec<(String, String)>) -> String {
    let mut args = Vec::new();
    let mut path = String::new();
    let mut dynamic = false;
    for s in &e.route.segs {
        match s {
            Seg::Static(n) => path.push_str(&format!("/{n}")),
            Seg::Param(n, _) | Seg::Optional(n, _) | Seg::Rest(n) => {
                let mut a: String = n
                    .chars()
                    .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
                    .collect();
                if TS_TAKEN.contains(&a.as_str()) {
                    a.push('_');
                }
                args.push(format!("{a}: string | number"));
                path.push_str(&format!("/${{encodeURIComponent({a})}}"));
                dynamic = true;
            }
        }
    }
    let path = match (path.is_empty(), dynamic) {
        (true, _) => "\"/\"".to_string(),
        (false, true) => format!("`{path}`"),
        (false, false) => q(&path),
    };
    let mut fields = Vec::new();
    let mut query = Vec::new();
    let mut body = None;
    let mut query_required = false;
    for (name, ty) in f.inputs().unwrap_or_default() {
        let t: String = ty.chars().filter(|c| !c.is_whitespace()).collect();
        match place(e, f, name, &t) {
            Place::Body => {
                let t = input(&t, e.types, decls);
                // A PATCH sends only what changes.
                body = Some(match f.name.as_str() {
                    "patch" if t.ends_with("Input") => format!("Partial<{t}>"),
                    _ => t,
                });
            }
            Place::Path => {}
            Place::Field => fields.push(member(name, &t, optional(&t), e.types, decls)),
            Place::Query => {
                query_required |= !optional(&t);
                query.push(member(name, &t, optional(&t), e.types, decls));
            }
        }
    }
    let body =
        body.or_else(|| (!fields.is_empty()).then(|| format!("{{ {} }}", fields.join("; "))));
    let mut call = vec![q(&f.name.to_uppercase()), path];
    if body.is_some() || !query.is_empty() {
        call.push(
            if query.is_empty() {
                "undefined"
            } else {
                "query"
            }
            .into(),
        );
    }
    if let Some(body) = body {
        args.push(format!("body: {body}"));
        call.push("body".into());
    }
    if !query.is_empty() {
        let mark = if query_required { "" } else { "?" };
        args.push(format!("query{mark}: {{ {} }}", query.join("; ")));
    }
    let returns = match f.returns_kind() {
        Returns::Nothing => "void".to_string(),
        Returns::Response | Returns::MaybeResponse => "unknown".into(),
        Returns::Other => match f.optional_value() {
            Some("()") => "void".into(),
            Some(t) => ts(&t.replace(char::is_whitespace, ""), e.types, decls),
            None => ts(
                &f.value_type().replace(char::is_whitespace, ""),
                e.types,
                decls,
            ),
        },
    };
    format!(
        "    {}: async ({}) => call<{returns}>({}),\n",
        camel(&operation_id(e, f)),
        args.join(", "),
        call.join(", ")
    )
}

/// `get_api_notes_id` → `getApiNotesId`.
fn camel(id: &str) -> String {
    let mut out = String::new();
    for (i, word) in id.split('_').filter(|w| !w.is_empty()).enumerate() {
        let mut chars = word.chars();
        if let Some(c) = chars.next() {
            out.push(if i == 0 { c } else { c.to_ascii_uppercase() });
            out.push_str(chars.as_str());
        }
    }
    out
}

/// A member of a TypeScript object type: `name?: T`.
fn member(
    name: &str,
    t: &str,
    optional: bool,
    types: &[TypeItem],
    decls: &mut Vec<(String, String)>,
) -> String {
    let name = name.strip_prefix("r#").unwrap_or(name);
    let mark = if optional { "?" } else { "" };
    format!("{name}{mark}: {}", ts(t, types, decls))
}

/// The TypeScript type of the Rust type `t` (no whitespace). Types defined
/// in the file are declared in `decls` and referred to; any other type is
/// `unknown`.
fn ts(t: &str, types: &[TypeItem], decls: &mut Vec<(String, String)>) -> String {
    let t = unref(t);
    let arg = || inner(t).unwrap_or("");
    let array = |item: String| {
        if item.contains(" | ") || item.contains(" & ") {
            format!("({item})[]")
        } else {
            format!("{item}[]")
        }
    };
    match rust_scan::last_segment(t) {
        "Box" | "Arc" | "Rc" => ts(arg(), types, decls),
        "Option" => format!("{} | null", ts(arg(), types, decls)),
        "Row" if inner(t).is_some() => format!("{{ id: number }} & {}", ts(arg(), types, decls)),
        "Vec" | "VecDeque" | "BTreeSet" | "HashSet" => array(ts(arg(), types, decls)),
        "BTreeMap" | "HashMap" => {
            let value = arg().split_once(',').map_or("", |(_, v)| v);
            format!("Record<string, {}>", ts(value, types, decls))
        }
        "String" | "str" | "char" | "Cow" => "string".into(),
        "bool" => "boolean".into(),
        "u8" | "u16" | "u32" | "u64" | "u128" | "usize" | "i8" | "i16" | "i32" | "i64" | "i128"
        | "isize" | "f32" | "f64" => "number".into(),
        "()" => "null".into(),
        _ if t.starts_with('[') => {
            let item = t[1..].split([';', ']']).next().unwrap_or("");
            array(ts(item, types, decls))
        }
        name => match types.iter().find(|ty| ty.name == name && !t.contains('<')) {
            Some(ty) => {
                if !decls.iter().any(|(n, _)| n == name) {
                    // Placed first, so a type that contains itself refers back.
                    decls.push((name.to_string(), String::new()));
                    let at = decls.len() - 1;
                    decls[at].1 = declare(ty, types, decls);
                }
                name.to_string()
            }
            None => "unknown".into(),
        },
    }
}

/// A struct defined in the file as an `interface`. One with no named fields
/// (an enum, a tuple struct) is `unknown`.
fn declare(ty: &TypeItem, types: &[TypeItem], decls: &mut Vec<(String, String)>) -> String {
    if ty.fields.is_empty() {
        return format!("export type {} = unknown;\n", ty.name);
    }
    let mut out = format!("export interface {} {{\n", ty.name);
    for (name, fty) in &ty.fields {
        let t: String = fty.chars().filter(|c| !c.is_whitespace()).collect();
        let optional = rust_scan::last_segment(&t) == "Option";
        out.push_str(&format!(
            "  {};\n",
            member(name, &t, optional, types, decls)
        ));
    }
    out.push_str("}\n");
    out
}

/// Whether a request may leave out field `name` (of type `t`, no
/// whitespace) of `ty`: `None`, an empty list and `false` are read for it,
/// and a `#[derive(Rest)]` type's `created_at` and `updated_at` are Wisp's.
fn may_leave_out(ty: &TypeItem, name: &str, t: &str) -> bool {
    matches!(rust_scan::last_segment(t), "Option" | "Vec")
        || t == "bool"
        || (ty.derives.iter().any(|d| d == "Rest") && matches!(name, "created_at" | "updated_at"))
}

/// A request body's type. A struct of the file with members a request may
/// leave out (other than `Option`s, optional already) is `NoteInput`:
/// `Note` with those optional.
fn input(t: &str, types: &[TypeItem], decls: &mut Vec<(String, String)>) -> String {
    let name = rust_scan::last_segment(unref(t));
    let Some(ty) = types.iter().find(|ty| {
        ty.name == name
            && !t.contains('<')
            && ty.fields.iter().any(|(f, fty)| {
                let t: String = fty.chars().filter(|c| !c.is_whitespace()).collect();
                may_leave_out(ty, f, &t) && rust_scan::last_segment(&t) != "Option"
            })
    }) else {
        return ts(t, types, decls);
    };
    let input = format!("{name}Input");
    if !decls.iter().any(|(n, _)| *n == input) {
        decls.push((input.clone(), String::new()));
        let at = decls.len() - 1;
        let mut out = format!("export interface {input} {{\n");
        for (field, fty) in &ty.fields {
            let t: String = fty.chars().filter(|c| !c.is_whitespace()).collect();
            let optional = may_leave_out(ty, field, &t);
            out.push_str(&format!(
                "  {};\n",
                member(field, &t, optional, types, decls)
            ));
        }
        out.push_str("}\n");
        decls[at].1 = out;
    }
    input
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
            member: false,
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
            "\"Note\":{\"type\":\"object\",\"required\":[\"id\",\"title\",\"author\"]",
            "\"due\":{\"type\":\"string\"},\"author\":{\"description\":\"crate::User\"}",
            "\"Error\":{\"type\":\"object\",\"required\":[\"status\",\"code\",\"error\"]",
        ] {
            assert!(json.contains(want), "{want}\n{json}");
        }
    }

    fn notes_route() -> Route {
        Route {
            dir: PathBuf::new(),
            segs: vec![
                Seg::Static("api".into()),
                Seg::Static("notes".into()),
                Seg::Param("id".into(), None),
            ],
            page: false,
            page_rs: false,
            page_js: false,
            server: true,
            member: false,
            layouts: Vec::new(),
            error: None,
        }
    }

    #[test]
    fn a_row_is_its_type_with_an_id() {
        let items = rust_scan::scan("struct Note { title: String }").unwrap();
        let mut schemas = Vec::new();
        let got = schema("Row<Note>", &items.types, &mut schemas);
        assert_eq!(
            got,
            "{\"allOf\":[{\"type\":\"object\",\"required\":[\"id\"],\"properties\":{\"id\":{\"type\":\"integer\",\"minimum\":0}}},{\"$ref\":\"#/components/schemas/Note\"}]}"
        );
        assert_eq!(schemas[0].0, "Note");
    }

    #[test]
    fn typescript_types_and_methods() {
        let items = rust_scan::scan(
            "struct Note { id: u64, title: String, tags: Vec<String>, due: Option<String>, meta: HashMap<String, Value>, r#type: char, kind: Kind }\n\
             enum Kind { A, B }\n\
             struct New { title: String }\n\
             fn get(q: Option<String>, limit: u32) -> Vec<Row<Note>> { todo!() }\n\
             async fn post(body: New) -> Result<Row<Note>> { todo!() }\n\
             fn put(id: u64, title: String, done: bool) -> Option<Response> { None }\n\
             fn patch(cx: &mut Cx, tags: Vec<String>) -> Option<()> { None }\n\
             fn delete(id: u64) {}",
        )
        .unwrap();
        let route = notes_route();
        let ts = typescript(&[Endpoint {
            route: &route,
            fns: &items.fns,
            types: &items.types,
        }]);
        let path = "`/api/notes/${encodeURIComponent(id)}`";
        for want in [
            "// Generated by Wisp from the app's +server.rs files. Do not edit.\n",
            "export interface Note {\n  id: number;\n  title: string;\n  tags: string[];\n  due?: string | null;\n  meta: Record<string, unknown>;\n  type: string;\n  kind: Kind;\n}\n",
            "export type Kind = unknown;\n",
            "export interface New {\n  title: string;\n}\n",
            "export interface ApiError {\n  status: number;\n  code: string;\n  error: string;\n  errors?: Record<string, string>;\n}",
            "export class WispError extends Error {",
            "export interface Options {\n  base?: string;\n  token?: string;",
            "export function client(options: Options = {}) {",
            &format!(
                "    getApiNotesId: async (id: string | number, query: {{ q?: string | null; limit: number }}) => call<({{ id: number }} & Note)[]>(\"GET\", {path}, query),\n"
            ),
            &format!(
                "    postApiNotesId: async (id: string | number, body: New) => call<{{ id: number }} & Note>(\"POST\", {path}, undefined, body),\n"
            ),
            &format!(
                "    putApiNotesId: async (id: string | number, body: {{ title: string; done?: boolean }}) => call<unknown>(\"PUT\", {path}, undefined, body),\n"
            ),
            &format!(
                "    patchApiNotesId: async (id: string | number, body: {{ tags?: string[] }}) => call<void>(\"PATCH\", {path}, undefined, body),\n"
            ),
            &format!(
                "    deleteApiNotesId: async (id: string | number) => call<void>(\"DELETE\", {path}),\n"
            ),
        ] {
            assert!(ts.contains(want), "{want}\n{ts}");
        }
        assert!(ts.ends_with("  };\n}\n"));
        assert_eq!(ts.matches("export interface Note ").count(), 1);

        // What a request may leave out is optional in what it sends.
        let items = rust_scan::scan(
            "#[derive(Rest)] struct Task { title: String, done: bool, tags: Vec<String>, created_at: String }\n\
             fn post(body: Task) -> Row<Task> { todo!() }\n\
             fn patch(id: u64, body: Task) -> Row<Task> { todo!() }",
        )
        .unwrap();
        let ts = typescript(&[Endpoint {
            route: &route,
            fns: &items.fns,
            types: &items.types,
        }]);
        for want in [
            "export interface TaskInput {\n  title: string;\n  done?: boolean;\n  tags?: string[];\n  created_at?: string;\n}\n",
            "(id: string | number, body: TaskInput) => call<{ id: number } & Task>",
            "(id: string | number, body: Partial<TaskInput>) =>",
            "export interface Task {\n  title: string;\n  done: boolean;",
        ] {
            assert!(ts.contains(want), "{want}\n{ts}");
        }
    }

    #[test]
    fn typescript_without_parameters() {
        let items = rust_scan::scan("fn get() -> String { todo!() }").unwrap();
        let mut route = notes_route();
        route.segs.truncate(2);
        let ts = typescript(&[Endpoint {
            route: &route,
            fns: &items.fns,
            types: &items.types,
        }]);
        assert!(
            ts.contains("    getApiNotes: async () => call<string>(\"GET\", \"/api/notes\"),\n"),
            "{ts}"
        );
    }

    #[test]
    fn validate_rules_become_limits() {
        let items = rust_scan::scan(
            "#[derive(FromJson)]
struct New {
    #[validate(len = 1..=100, email)]
    to: String,
    #[validate(min = 1, max = 10)]
    n: u8,
    #[validate(len = ..5)]
    tags: Vec<String>,
    #[validate(max_len = MAX)]
    x: String,
}
fn post(body: New) {}",
        )
        .unwrap();
        let mut schemas = Vec::new();
        let got = schema("New", &items.types, &mut schemas);
        assert_eq!(got, "{\"$ref\":\"#/components/schemas/New\"}");
        for want in [
            "\"to\":{\"type\":\"string\",\"minLength\":1,\"maxLength\":100,\"format\":\"email\"}",
            "\"n\":{\"type\":\"integer\",\"minimum\":1,\"maximum\":10}",
            "\"tags\":{\"type\":\"array\",\"items\":{\"type\":\"string\"},\"maxItems\":4}",
            "\"x\":{\"type\":\"string\"}",
        ] {
            assert!(
                schemas[0].1.contains(want),
                "{want}
{}",
                schemas[0].1
            );
        }
    }
}
