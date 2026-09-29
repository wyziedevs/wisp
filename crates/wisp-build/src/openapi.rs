//! An OpenAPI 3.1 description of the app's `+server.rs` endpoints, made at
//! build time from what the build already reads: each route's path, the
//! methods its file defines, their inputs by name and type, and what they
//! return. Types defined in the same `+server.rs` are described field by
//! field; any other type is named in a description and left open, rather
//! than guessed. `wisp` serves it at `/_wisp/openapi.json`, with a page to
//! try it at `/_wisp/docs`. The same data makes a TypeScript client, one
//! module with a typed method per endpoint.

use crate::json_str as q;
use crate::routes::{Route, Seg};
use crate::rules::{self, Key};
use crate::rust_scan::{self, FnItem, Returns, TypeItem};
use crate::ty::{self, Scalar, inner, last_segment, option_inner, squeeze, unref};

/// One operation, as the document and the client describe it: a function
/// of a `+server.rs`, or one its `#[derive(Rest)]` type answers.
#[derive(Clone)]
pub struct Op {
    /// `get`, `post`, `put`, `patch` or `delete`.
    pub method: &'static str,
    /// What it reads from the request, by name: (name, type).
    pub inputs: Vec<(String, String)>,
    /// What it returns, through a `Result`.
    pub value: String,
    /// It returns a `Result`: it may fail with any error.
    pub fallible: bool,
}

impl Op {
    /// The operation `f` is, answering `method`.
    pub fn of(method: &'static str, f: &FnItem) -> Op {
        let inputs = f.inputs().unwrap_or_default();
        Op {
            method,
            inputs: inputs
                .into_iter()
                .map(|(n, t)| (n.to_string(), t.to_string()))
                .collect(),
            value: f.value_type().to_string(),
            fallible: f.fallible,
        }
    }

    fn returns(&self) -> Returns {
        rust_scan::returns_kind(&self.value)
    }

    /// `T` of an `Option<T>` it returns (not `Option<Response>`).
    fn optional(&self) -> Option<String> {
        match self.returns() {
            Returns::Other => option_inner(&self.value).map(squeeze),
            _ => None,
        }
    }
}

/// One `+server.rs`: its route, its operations, and the types it can
/// describe.
pub struct Endpoint<'a> {
    pub route: &'a Route,
    pub ops: &'a [Op],
    pub types: &'a [TypeItem],
}

/// A JSON value, built whole and written once.
enum J {
    Obj(Vec<(String, J)>),
    Arr(Vec<J>),
    Str(String),
    Num(String),
    Bool(bool),
    /// JSON text as it is.
    Raw(&'static str),
}

impl J {
    fn obj<const N: usize>(pairs: [(&str, J); N]) -> J {
        J::Obj(pairs.into_iter().map(|(k, v)| (k.to_string(), v)).collect())
    }

    fn str(s: &str) -> J {
        J::Str(s.to_string())
    }

    fn of(ty: &str) -> J {
        J::obj([("type", J::str(ty))])
    }

    /// Sets `key` of an object, in place if it has it.
    fn set(&mut self, key: &str, value: J) {
        let J::Obj(m) = self else { return };
        match m.iter_mut().find(|(k, _)| k == key) {
            Some((_, v)) => *v = value,
            None => m.push((key.to_string(), value)),
        }
    }

    fn write(&self, out: &mut String) {
        match self {
            J::Obj(m) => {
                out.push('{');
                for (k, (key, v)) in m.iter().enumerate() {
                    if k > 0 {
                        out.push(',');
                    }
                    out.push_str(&q(key));
                    out.push(':');
                    v.write(out);
                }
                out.push('}');
            }
            J::Arr(a) => {
                out.push('[');
                for (k, v) in a.iter().enumerate() {
                    if k > 0 {
                        out.push(',');
                    }
                    v.write(out);
                }
                out.push(']');
            }
            J::Str(s) => out.push_str(&q(s)),
            J::Num(n) => out.push_str(n),
            J::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
            J::Raw(s) => out.push_str(s),
        }
    }
}

/// The error body every endpoint's failures have.
const ERROR: &str = r#"{"type":"object","required":["status","code","error"],"properties":{"status":{"type":"integer"},"code":{"type":"string"},"error":{"type":"string"},"errors":{"type":"object","additionalProperties":{"type":"string"},"description":"What is wrong, by field (a 422)"}}}"#;

/// The whole document, as JSON.
pub fn spec(title: &str, version: &str, endpoints: &[Endpoint]) -> String {
    let mut schemas: Vec<(String, J)> = Vec::new();
    let mut paths = Vec::new();
    for e in endpoints {
        let ops = e
            .ops
            .iter()
            .map(|op| (op.method.to_string(), operation(e, op, &mut schemas)))
            .collect();
        paths.push((path(e.route), J::Obj(ops)));
    }
    schemas.push(("Error".into(), J::Raw(ERROR)));
    let doc = J::obj([
        ("openapi", J::str("3.1.0")),
        (
            "info",
            J::obj([("title", J::str(title)), ("version", J::str(version))]),
        ),
        ("paths", J::Obj(paths)),
        ("components", J::obj([("schemas", J::Obj(schemas))])),
    ]);
    let mut out = String::new();
    doc.write(&mut out);
    out
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

/// The names of the route's parameters, in order.
fn route_params(r: &Route) -> impl Iterator<Item = &str> {
    r.segs.iter().filter_map(|s| match s {
        Seg::Static(_) => None,
        Seg::Param(n, _) | Seg::Optional(n, _) | Seg::Rest(n) => Some(n.as_str()),
    })
}

fn operation(e: &Endpoint, op: &Op, schemas: &mut Vec<(String, J)>) -> J {
    let mut params = Vec::new();
    let mut in_path: Vec<&str> = Vec::new();
    let mut fields = Vec::new();
    let mut required = Vec::new();
    let mut body = None;
    let param = |name: &str, place: &str, required: bool, schema: J| {
        J::obj([
            ("name", J::str(name)),
            ("in", J::str(place)),
            ("required", J::Bool(required)),
            ("schema", schema),
        ])
    };
    for (name, t) in &op.inputs {
        let optional = may_omit(t);
        let schema = schema(t, e.types, schemas);
        match place(e, op, name, t) {
            Place::Body => body = Some(schema),
            Place::Path => {
                in_path.push(name);
                params.push(param(name, "path", true, schema));
            }
            Place::Field => {
                fields.push((name.clone(), schema));
                if !optional {
                    required.push(J::str(name));
                }
            }
            Place::Query => params.push(param(name, "query", !optional, schema)),
        }
    }
    // Path parameters the function does not take are still in the path.
    for n in route_params(e.route) {
        if !in_path.contains(&n) {
            params.push(param(n, "path", true, J::of("string")));
        }
    }
    let mut out = J::obj([("operationId", J::str(&operation_id(e, op)))]);
    if !params.is_empty() {
        out.set("parameters", J::Arr(params));
    }
    let content = |schema: J| J::obj([("schema", schema)]);
    let request = match body {
        Some(schema) => Some(J::obj([("application/json", content(schema))])),
        None if !fields.is_empty() => {
            let schema = || {
                J::obj([
                    ("type", J::str("object")),
                    ("required", J::Arr(required.iter().map(clone).collect())),
                    (
                        "properties",
                        J::Obj(fields.iter().map(|(n, s)| (n.clone(), clone(s))).collect()),
                    ),
                ])
            };
            Some(J::obj([
                ("application/json", content(schema())),
                ("application/x-www-form-urlencoded", content(schema())),
            ]))
        }
        None => None,
    };
    let validated = request.is_some();
    if let Some(content) = request {
        out.set(
            "requestBody",
            J::obj([("required", J::Bool(true)), ("content", content)]),
        );
    }
    let said = |what: &str| J::obj([("description", J::str(what))]);
    let json = |what: &str, schema: J| {
        J::obj([
            ("description", J::str(what)),
            ("content", J::obj([("application/json", content(schema))])),
        ])
    };
    let mut responses: Vec<(String, J)> = match op.returns() {
        Returns::Nothing => vec![("204".into(), said("Done"))],
        Returns::Response => vec![("200".into(), said("A response"))],
        Returns::MaybeResponse => vec![
            ("200".into(), said("A response")),
            ("404".into(), said("Not found")),
        ],
        Returns::Other => match op.optional().as_deref() {
            Some("()") => vec![
                ("204".into(), said("Done")),
                ("404".into(), said("Not found")),
            ],
            Some(t) => vec![
                ("200".into(), json("OK", schema(t, e.types, schemas))),
                ("404".into(), said("Not found")),
            ],
            None => vec![(
                "200".into(),
                json("OK", schema(&squeeze(&op.value), e.types, schemas)),
            )],
        },
    };
    let error = || J::obj([("$ref", J::str("#/components/schemas/Error"))]);
    if validated {
        responses.push(("422".into(), json("The input did not pass", error())));
    }
    if op.fallible {
        responses.push(("default".into(), json("An error", error())));
    }
    out.set("responses", J::Obj(responses));
    out
}

/// A copy of `j` (the document is built once; this is for the two
/// content types a form body is sent as).
fn clone(j: &J) -> J {
    match j {
        J::Obj(m) => J::Obj(m.iter().map(|(k, v)| (k.clone(), clone(v))).collect()),
        J::Arr(a) => J::Arr(a.iter().map(clone).collect()),
        J::Str(s) => J::Str(s.clone()),
        J::Num(n) => J::Num(n.clone()),
        J::Bool(b) => J::Bool(*b),
        J::Raw(s) => J::Raw(s),
    }
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

fn place(e: &Endpoint, op: &Op, name: &str, t: &str) -> Place {
    if name == "body" && !ty::is_maybe_text(t) {
        Place::Body
    } else if route_params(e.route).any(|n| n == name) {
        Place::Path
    } else if matches!(op.method, "post" | "put" | "patch") {
        Place::Field
    } else {
        Place::Query
    }
}

/// Whether a request may leave the input (of type `t`) out: `None`, an
/// empty list and `false` are read for it.
fn may_omit(t: &str) -> bool {
    matches!(last_segment(t), "Option" | "Vec") || ty::scalar(t) == Scalar::Bool
}

/// `get_api_notes_id`: the method and the route.
fn operation_id(e: &Endpoint, op: &Op) -> String {
    format!("{}{}", op.method, id(e.route))
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

/// The JSON Schema of the Rust type `t`. Types defined in the file are
/// added to `schemas` and referred to.
fn schema(t: &str, types: &[TypeItem], schemas: &mut Vec<(String, J)>) -> J {
    let t = squeeze(t);
    let t = unref(&t);
    let arg = || inner(t).unwrap_or("");
    let array = |items: J| J::obj([("type", J::str("array")), ("items", items)]);
    match ty::scalar(t) {
        Scalar::Text | Scalar::Char => return J::of("string"),
        Scalar::Bool => return J::of("boolean"),
        Scalar::Unsigned => {
            return J::obj([("type", J::str("integer")), ("minimum", J::Num("0".into()))]);
        }
        Scalar::Signed => return J::of("integer"),
        Scalar::Float => return J::of("number"),
        Scalar::Unit => return J::of("null"),
        Scalar::Other => {}
    }
    match last_segment(t) {
        "Option" | "Box" | "Arc" | "Rc" => schema(arg(), types, schemas),
        "Vec" | "VecDeque" | "BTreeSet" | "HashSet" => array(schema(arg(), types, schemas)),
        "BTreeMap" | "HashMap" => {
            let value = arg().split_once(',').map_or("", |(_, v)| v);
            J::obj([
                ("type", J::str("object")),
                ("additionalProperties", schema(value, types, schemas)),
            ])
        }
        // Wisp's row: the fields of `T`, `id` first.
        "Row" if inner(t).is_some() => {
            let id = J::obj([
                ("type", J::str("object")),
                ("required", J::Arr(vec![J::str("id")])),
                (
                    "properties",
                    J::obj([(
                        "id",
                        J::obj([("type", J::str("integer")), ("minimum", J::Num("0".into()))]),
                    )]),
                ),
            ]);
            J::obj([("allOf", J::Arr(vec![id, schema(arg(), types, schemas)]))])
        }
        "Value" => J::Obj(Vec::new()),
        _ if t.starts_with('[') => {
            let item = t[1..].split([';', ']']).next().unwrap_or("");
            array(schema(item, types, schemas))
        }
        _ if t.starts_with('(') => J::of("array"),
        name => match types.iter().find(|ty| ty.name == name && !t.contains('<')) {
            Some(ty) => {
                if !schemas.iter().any(|(n, _)| n == name) {
                    // Placed first, so a type that contains itself refers back.
                    schemas.push((name.to_string(), J::Obj(Vec::new())));
                    let at = schemas.len() - 1;
                    schemas[at].1 = object(ty, types, schemas);
                }
                J::obj([("$ref", J::Str(format!("#/components/schemas/{name}")))])
            }
            None => J::obj([("description", J::str(t))]),
        },
    }
}

/// `schema` with the limits of a field's `#[validate(…)]`: `min`, `max`,
/// `min_len`, `max_len`, `len` (a range) and `email`. A limit that is not a
/// plain number (a constant, say) is left out, not guessed.
fn constrain(schema: &mut J, rules: &str, t: &str) {
    let (min_len, max_len) = match last_segment(t) {
        "Vec" => ("minItems", "maxItems"),
        _ => ("minLength", "maxLength"),
    };
    let mut add = |key: &str, n: &str| {
        if n.parse::<f64>().is_ok() {
            schema.set(key, J::Num(n.to_string()));
        }
    };
    let mut email = false;
    for r in rules::parse(rules).unwrap_or_default() {
        match r.key {
            Key::Min => add("minimum", r.value),
            Key::Max => add("maximum", r.value),
            Key::MinLen => add(min_len, r.value),
            Key::MaxLen => add(max_len, r.value),
            Key::Len => {
                let (lo, hi) = r.len_bounds();
                add(min_len, &lo.map_or(String::new(), |n| n.to_string()));
                add(max_len, &hi.map_or(String::new(), |n| n.to_string()));
            }
            Key::Email => email = true,
        }
    }
    if email {
        schema.set("format", J::str("email"));
    }
}

/// A struct defined in the file, field by field. One with no named fields
/// (an enum, a tuple struct) is only named.
fn object(ty: &TypeItem, types: &[TypeItem], schemas: &mut Vec<(String, J)>) -> J {
    if ty.fields.is_empty() {
        return J::obj([("title", J::str(&ty.name))]);
    }
    let mut props = Vec::new();
    let mut required = Vec::new();
    for (name, t) in &ty.fields {
        let name = name.strip_prefix("r#").unwrap_or(name);
        let mut schema = schema(t, types, schemas);
        if let Some((_, rules)) = ty.rules.iter().find(|(f, _)| f == name) {
            constrain(&mut schema, rules, t);
        }
        props.push((name.to_string(), schema));
        if !may_leave_out(ty, name, t) {
            required.push(J::str(name));
        }
    }
    J::obj([
        ("type", J::str("object")),
        ("required", J::Arr(required)),
        ("properties", J::Obj(props)),
    ])
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
        for op in e.ops {
            methods.push_str(&method(e, op, &mut decls));
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
fn method(e: &Endpoint, op: &Op, decls: &mut Vec<(String, String)>) -> String {
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
    for (name, t) in &op.inputs {
        match place(e, op, name, t) {
            Place::Body => {
                let t = input(t, e.types, decls);
                // A PATCH sends only what changes.
                body = Some(match op.method {
                    "patch" if t.ends_with("Input") => format!("Partial<{t}>"),
                    _ => t,
                });
            }
            Place::Path => {}
            Place::Field => fields.push(member(name, t, may_omit(t), e.types, decls)),
            Place::Query => {
                query_required |= !may_omit(t);
                query.push(member(name, t, may_omit(t), e.types, decls));
            }
        }
    }
    let body =
        body.or_else(|| (!fields.is_empty()).then(|| format!("{{ {} }}", fields.join("; "))));
    let mut call = vec![q(&op.method.to_uppercase()), path];
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
    let returns = match op.returns() {
        Returns::Nothing => "void".to_string(),
        Returns::Response | Returns::MaybeResponse => "unknown".into(),
        Returns::Other => match op.optional().as_deref() {
            Some("()") => "void".into(),
            Some(t) => ts(t, e.types, decls),
            None => ts(&op.value, e.types, decls),
        },
    };
    format!(
        "    {}: async ({}) => call<{returns}>({}),\n",
        camel(&operation_id(e, op)),
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

/// The TypeScript type of the Rust type `t`. Types defined in the file are
/// declared in `decls` and referred to; any other type is `unknown`.
fn ts(t: &str, types: &[TypeItem], decls: &mut Vec<(String, String)>) -> String {
    let t = squeeze(t);
    let t = unref(&t);
    let arg = || inner(t).unwrap_or("");
    let array = |item: String| {
        if item.contains(" | ") || item.contains(" & ") {
            format!("({item})[]")
        } else {
            format!("{item}[]")
        }
    };
    match ty::scalar(t) {
        Scalar::Text | Scalar::Char => return "string".into(),
        Scalar::Bool => return "boolean".into(),
        Scalar::Unsigned | Scalar::Signed | Scalar::Float => return "number".into(),
        Scalar::Unit => return "null".into(),
        Scalar::Other => {}
    }
    match last_segment(t) {
        "Box" | "Arc" | "Rc" => ts(arg(), types, decls),
        "Option" => format!("{} | null", ts(arg(), types, decls)),
        "Row" if inner(t).is_some() => format!("{{ id: number }} & {}", ts(arg(), types, decls)),
        "Vec" | "VecDeque" | "BTreeSet" | "HashSet" => array(ts(arg(), types, decls)),
        "BTreeMap" | "HashMap" => {
            let value = arg().split_once(',').map_or("", |(_, v)| v);
            format!("Record<string, {}>", ts(value, types, decls))
        }
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
                    decls[at].1 = declare(ty, types, decls, None);
                }
                name.to_string()
            }
            None => "unknown".into(),
        },
    }
}

/// A struct defined in the file as an `interface` (named `name`, or its
/// own), whose members are optional as `optional` says: by default, its
/// `Option`s. One with no named fields (an enum, a tuple struct) is
/// `unknown`.
fn declare(
    ty: &TypeItem,
    types: &[TypeItem],
    decls: &mut Vec<(String, String)>,
    input: Option<&str>,
) -> String {
    if ty.fields.is_empty() {
        return format!("export type {} = unknown;\n", ty.name);
    }
    let mut out = format!("export interface {} {{\n", input.unwrap_or(&ty.name));
    for (name, t) in &ty.fields {
        let optional = match input {
            Some(_) => may_leave_out(ty, name, t),
            None => last_segment(t) == "Option",
        };
        out.push_str(&format!("  {};\n", member(name, t, optional, types, decls)));
    }
    out.push_str("}\n");
    out
}

/// Whether a request may leave out field `name` (of type `t`) of `ty`:
/// `None`, an empty list and `false` are read for it, and a
/// `#[derive(Rest)]` type's `created_at` and `updated_at` are Wisp's.
fn may_leave_out(ty: &TypeItem, name: &str, t: &str) -> bool {
    may_omit(t)
        || (ty.derives.iter().any(|d| d == "Rest") && matches!(name, "created_at" | "updated_at"))
}

/// A request body's type. A struct of the file with members a request may
/// leave out (other than `Option`s, optional already) is `NoteInput`:
/// `Note` with those optional.
fn input(t: &str, types: &[TypeItem], decls: &mut Vec<(String, String)>) -> String {
    let name = last_segment(unref(t));
    let Some(ty) = types.iter().find(|ty| {
        ty.name == name
            && !t.contains('<')
            && ty
                .fields
                .iter()
                .any(|(f, t)| may_leave_out(ty, f, t) && last_segment(t) != "Option")
    }) else {
        return ts(t, types, decls);
    };
    let input = format!("{name}Input");
    if !decls.iter().any(|(n, _)| *n == input) {
        decls.push((input.clone(), String::new()));
        let at = decls.len() - 1;
        decls[at].1 = declare(ty, types, decls, Some(&input));
    }
    input
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn ops(items: &rust_scan::Items) -> Vec<Op> {
        let method = |n: &str| {
            ["get", "post", "put", "patch", "delete"]
                .into_iter()
                .find(|m| *m == n)
                .expect("a method")
        };
        items
            .fns
            .iter()
            .map(|f| Op::of(method(&f.name), f))
            .collect()
    }

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
                ops: &ops(&items),
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

    fn text(j: &J) -> String {
        let mut out = String::new();
        j.write(&mut out);
        out
    }

    #[test]
    fn a_row_is_its_type_with_an_id() {
        let items = rust_scan::scan("struct Note { title: String }").unwrap();
        let mut schemas = Vec::new();
        let got = text(&schema("Row<Note>", &items.types, &mut schemas));
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
            ops: &ops(&items),
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
            ops: &ops(&items),
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
            ops: &ops(&items),
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
        let got = text(&schema("New", &items.types, &mut schemas));
        assert_eq!(got, "{\"$ref\":\"#/components/schemas/New\"}");
        let new = text(&schemas[0].1);
        for want in [
            "\"to\":{\"type\":\"string\",\"minLength\":1,\"maxLength\":100,\"format\":\"email\"}",
            "\"n\":{\"type\":\"integer\",\"minimum\":1,\"maximum\":10}",
            "\"tags\":{\"type\":\"array\",\"items\":{\"type\":\"string\"},\"maxItems\":4}",
            "\"x\":{\"type\":\"string\"}",
        ] {
            assert!(new.contains(want), "{want}\n{new}");
        }
    }
}
