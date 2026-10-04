//! An OpenAPI 3.1 description of the app, made at build time from what the
//! build already reads: each `+server.rs` endpoint (its path, the methods
//! its file defines or its `#[derive(Rest)]` type answers, their inputs by
//! name and type, what they return and fail with), and each page and its
//! form actions. Types defined in the same file are described field by
//! field (an enum without fields by its names); any other type is named in a
//! description and left open, rather than guessed. Security is what the
//! code visibly asks for: a `#[rest(...)]` key, `need_bearer`, `signed_in`.
//! `wisp` serves it at `/_wisp/openapi.json`, with a page to try it at
//! `/_wisp/docs`, and `wisp openapi` prints it. The same data makes a
//! TypeScript client, one module with a typed method per endpoint.

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
    /// It asks for a bearer token.
    pub bearer: bool,
    /// It asks for a signed-in member.
    pub session: bool,
    /// What a `#[derive(Rest)]` type answers here (`list`, `create`, `get`,
    /// `put`, `patch` or `delete`) and the type; none for a function.
    pub rest: Option<(&'static str, String)>,
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
            bearer: f.bearer,
            session: f.session,
            rest: None,
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

/// The same as RFC 9457 problem details, for `accept: application/problem+json`.
const PROBLEM: &str = r#"{"type":"object","required":["status","code"],"properties":{"type":{"type":"string"},"title":{"type":"string"},"status":{"type":"integer"},"code":{"type":"string"},"detail":{"type":"string"},"errors":{"type":"object","additionalProperties":{"type":"string"}}}}"#;

/// What the document shares: the error answers every operation refers to.
const RESPONSES: &str = r##"{"Error":{"description":"An error: its status, a code, a message, and for a 422 what is wrong by field","content":{"application/json":{"schema":{"$ref":"#/components/schemas/Error"}},"application/problem+json":{"schema":{"$ref":"#/components/schemas/Problem"}}}},"Unauthorized":{"description":"No valid token or session","content":{"application/json":{"schema":{"$ref":"#/components/schemas/Error"}},"application/problem+json":{"schema":{"$ref":"#/components/schemas/Problem"}}}}}"##;

/// A page of the app and the form actions in its `+page.rs`.
pub struct Page<'a> {
    pub route: &'a Route,
    pub actions: &'a [Action],
    pub types: &'a [TypeItem],
}

/// An `#[action]` of a page: `default` answers the page's own address,
/// the others `?/name`.
#[derive(Clone)]
pub struct Action {
    pub name: String,
    /// What it reads from the form, route or query, by name: (name, type).
    pub inputs: Vec<(String, String)>,
    /// `#[validate(...)]` on its parameters: (parameter, rules).
    pub checks: Vec<(String, String)>,
    pub bearer: bool,
    pub session: bool,
}

impl Action {
    /// The action `f` is.
    pub fn of(f: &FnItem) -> Action {
        Action {
            name: f.name.clone(),
            inputs: (f.inputs().unwrap_or_default().into_iter())
                .map(|(n, t)| (n.to_string(), t.to_string()))
                .collect(),
            checks: f.checks.clone(),
            bearer: f.bearer,
            session: f.session,
        }
    }
}

/// What the operations of a document use, for what it declares.
#[derive(Default)]
struct Used {
    bearer: bool,
    session: bool,
}

/// The whole document, as JSON: the endpoints, then the pages and their
/// actions.
pub fn spec(title: &str, version: &str, endpoints: &[Endpoint], pages: &[Page]) -> String {
    let mut schemas: Vec<(String, J)> = Vec::new();
    let mut paths: Vec<(String, J)> = Vec::new();
    let mut used = Used::default();
    let add = |paths: &mut Vec<(String, J)>, key: String, method: &str, op: J| {
        let at = match paths.iter().position(|(k, _)| *k == key) {
            Some(at) => at,
            // Paths that differ only by their parameters' names (`/[n=int]`
            // and `/[slug]`) are one to OpenAPI: the first is described.
            None if paths.iter().any(|(k, _)| same_shape(k, &key)) => return,
            None => {
                paths.push((key, J::Obj(Vec::new())));
                paths.len() - 1
            }
        };
        if let J::Obj(m) = &mut paths[at].1
            && m.iter().all(|(k, _)| k != method)
        {
            m.push((method.to_string(), op));
        }
    };
    for e in endpoints {
        for &short in shorts(e.route) {
            for op in e.ops {
                let j = operation(e, op, short, &mut schemas, &mut used);
                add(&mut paths, path(e.route, short), op.method, j);
            }
        }
    }
    for pg in pages {
        for &short in shorts(pg.route) {
            let key = path(pg.route, short);
            add(&mut paths, key.clone(), "get", page(pg, short));
            for a in pg.actions {
                let at = match a.name.as_str() {
                    "default" => key.clone(),
                    n => format!("{key}?/{n}"),
                };
                let j = action(pg, a, short, &mut schemas, &mut used);
                add(&mut paths, at, "post", j);
            }
        }
    }
    schemas.push(("Error".into(), J::Raw(ERROR)));
    schemas.push(("Problem".into(), J::Raw(PROBLEM)));
    let mut components = vec![
        ("schemas".to_string(), J::Obj(schemas)),
        ("responses".to_string(), J::Raw(RESPONSES)),
    ];
    let mut schemes = Vec::new();
    if used.bearer {
        schemes.push((
            "bearer".to_string(),
            J::obj([("type", J::str("http")), ("scheme", J::str("bearer"))]),
        ));
    }
    if used.session {
        schemes.push((
            "session".to_string(),
            J::obj([
                ("type", J::str("apiKey")),
                ("in", J::str("cookie")),
                ("name", J::str("session")),
            ]),
        ));
    }
    if !schemes.is_empty() {
        components.push(("securitySchemes".into(), J::Obj(schemes)));
    }
    let doc = J::obj([
        ("openapi", J::str("3.1.0")),
        (
            "info",
            J::obj([("title", J::str(title)), ("version", J::str(version))]),
        ),
        ("paths", J::Obj(paths)),
        ("components", J::Obj(components)),
    ]);
    let mut out = String::new();
    doc.write(&mut out);
    out
}

/// Whether two paths are the same but for the names in their `{braces}`.
fn same_shape(a: &str, b: &str) -> bool {
    let bare = |p: &str| {
        let mut out = String::new();
        let mut inside = false;
        for c in p.chars() {
            match c {
                '{' => {
                    inside = true;
                    out.push_str("{}");
                }
                '}' => inside = false,
                _ if !inside => out.push(c),
                _ => {}
            }
        }
        out
    };
    bare(a) == bare(b)
}

/// `json` (as [`spec`] writes it: compact, valid) indented two spaces a
/// level, a member a line, for a file that is committed and diffed.
pub fn pretty(json: &str) -> String {
    let mut out = String::with_capacity(json.len() * 2);
    let (mut depth, mut quoted, mut escaped) = (0usize, false, false);
    let mut chars = json.chars().peekable();
    let line = |out: &mut String, depth: usize| {
        out.push('\n');
        out.extend(std::iter::repeat_n(' ', depth * 2));
    };
    while let Some(c) = chars.next() {
        if quoted {
            out.push(c);
            match (escaped, c) {
                (true, _) => escaped = false,
                (false, '\\') => escaped = true,
                (false, '"') => quoted = false,
                _ => {}
            }
            continue;
        }
        match c {
            '"' => {
                quoted = true;
                out.push(c);
            }
            '{' | '[' => {
                out.push(c);
                if matches!(chars.peek(), Some('}' | ']')) {
                    out.push(chars.next().unwrap_or(c));
                } else {
                    depth += 1;
                    line(&mut out, depth);
                }
            }
            '}' | ']' => {
                depth = depth.saturating_sub(1);
                line(&mut out, depth);
                out.push(c);
            }
            ',' => {
                out.push(c);
                line(&mut out, depth);
            }
            ':' => out.push_str(": "),
            c => out.push(c),
        }
    }
    out.push('\n');
    out
}

/// A route's optional and rest segments may be there or not, which are
/// two paths: the one with them, and (if it has any) the one without.
fn shorts(r: &Route) -> &'static [bool] {
    match (r.segs.iter()).any(|s| matches!(s, Seg::Optional(..) | Seg::Rest(_))) {
        true => &[false, true],
        false => &[false],
    }
}

/// `/notes/[id]` as OpenAPI writes it: `/notes/{id}`; with `short`, without
/// its optional and rest segments.
fn path(r: &Route, short: bool) -> String {
    let mut out = String::new();
    for s in kept(r, short) {
        match s {
            Seg::Static(n) => out.push_str(&format!("/{n}")),
            Seg::Param(n, _) | Seg::Optional(n, _) | Seg::Rest(n) => {
                out.push_str(&format!("/{{{n}}}"))
            }
        }
    }
    if out.is_empty() {
        out.push('/');
    }
    out
}

/// The segments of the path with `short` (see [`path`]) or without.
fn kept(r: &Route, short: bool) -> impl Iterator<Item = &Seg> {
    (r.segs.iter()).filter(move |s| !(short && matches!(s, Seg::Optional(..) | Seg::Rest(_))))
}

/// The parameters of the path, in order, with their matchers.
fn route_params(r: &Route, short: bool) -> impl Iterator<Item = (&str, Option<&str>)> {
    kept(r, short).filter_map(|s| match s {
        Seg::Static(_) => None,
        Seg::Param(n, m) | Seg::Optional(n, m) => Some((n.as_str(), m.as_deref())),
        Seg::Rest(n) => Some((n.as_str(), None)),
    })
}

/// A path parameter the function does not take (or take typed): `[id=int]`
/// is a whole number, any other a string.
fn segment(matcher: Option<&str>) -> J {
    match matcher {
        Some("int") => J::obj([("type", J::str("integer")), ("minimum", J::Num("0".into()))]),
        _ => J::of("string"),
    }
}

fn param(name: &str, place: &str, required: bool, schema: J) -> J {
    J::obj([
        ("name", J::str(name)),
        ("in", J::str(place)),
        ("required", J::Bool(required)),
        ("schema", schema),
    ])
}

/// `{"$ref": "#/components/<kind>/<name>"}`.
fn reference(kind: &str, name: &str) -> J {
    J::obj([("$ref", J::Str(format!("#/components/{kind}/{name}")))])
}

/// An operation's `security`: any one of the schemes it takes.
fn security(bearer: bool, session: bool, used: &mut Used) -> Option<J> {
    used.bearer |= bearer;
    used.session |= session;
    let one = |n: &str| J::obj([(n, J::Arr(Vec::new()))]);
    match (bearer, session) {
        (false, false) => None,
        (true, false) => Some(J::Arr(vec![one("bearer")])),
        (false, true) => Some(J::Arr(vec![one("session")])),
        (true, true) => Some(J::Arr(vec![one("bearer"), one("session")])),
    }
}

/// An answer of HTML.
fn html(what: &str) -> J {
    J::obj([
        ("description", J::str(what)),
        (
            "content",
            J::obj([("text/html", J::obj([("schema", J::of("string"))]))]),
        ),
    ])
}

/// The GET of a page: its HTML.
fn page(pg: &Page, short: bool) -> J {
    let params: Vec<J> = route_params(pg.route, short)
        .map(|(n, m)| param(n, "path", true, segment(m)))
        .collect();
    let mut out = J::obj([
        (
            "operationId",
            J::Str(format!("page{}", id(pg.route, short))),
        ),
        ("tags", J::Arr(vec![J::str("pages")])),
    ]);
    if !params.is_empty() {
        out.set("parameters", J::Arr(params));
    }
    out.set(
        "responses",
        J::obj([
            ("200", html("The page")),
            ("default", reference("responses", "Error")),
        ]),
    );
    out
}

/// A form action: a POST of a form, answered by the page again, or a
/// redirect.
fn action(
    pg: &Page,
    a: &Action,
    short: bool,
    schemas: &mut Vec<(String, J)>,
    used: &mut Used,
) -> J {
    let mut params = Vec::new();
    let mut props: Vec<(String, J)> = Vec::new();
    let mut required = Vec::new();
    let mut files = false;
    for (name, t) in &a.inputs {
        // The route gives it.
        if let Some((_, m)) = route_params(pg.route, false).find(|(n, _)| n == name) {
            if route_params(pg.route, short).any(|(n, _)| n == name) {
                params.push(param(name, "path", true, segment(m)));
            }
            continue;
        }
        // A struct is read by its fields: `fn default(post: Post)`.
        if let Some(ty) = unwrapped(t, pg.types)
            && !ty.fields.is_empty()
        {
            for (f, ft) in &ty.fields {
                let f = f.strip_prefix("r#").unwrap_or(f);
                let rules = ty.rules.iter().find(|(n, _)| n == f);
                let (s, file) = form_field(ft, rules, pg.types, schemas);
                files |= file;
                props.push((f.to_string(), s));
                if !may_leave_out(ty, f, ft) {
                    required.push(J::str(f));
                }
            }
            continue;
        }
        let rules = a.checks.iter().find(|(n, _)| n == name);
        let (s, file) = form_field(t, rules, pg.types, schemas);
        files |= file;
        props.push((name.clone(), s));
        if !may_omit(t) {
            required.push(J::str(name));
        }
    }
    // Path parameters the action does not take are still in the path.
    for (n, m) in route_params(pg.route, short) {
        let declared = |p: &J| matches!(p, J::Obj(o) if o.iter().any(|(k, v)| k == "name" && matches!(v, J::Str(s) if s == n)));
        if !params.iter().any(declared) {
            params.push(param(n, "path", true, segment(m)));
        }
    }
    let mut out = J::obj([
        (
            "operationId",
            J::Str(format!("action_{}{}", a.name, id(pg.route, short))),
        ),
        ("tags", J::Arr(vec![J::str("actions")])),
    ]);
    if !params.is_empty() {
        out.set("parameters", J::Arr(params));
    }
    let form = !props.is_empty();
    if form {
        let schema = J::obj([
            ("type", J::str("object")),
            ("required", J::Arr(required)),
            ("properties", J::Obj(props)),
        ]);
        let kind = match files {
            true => "multipart/form-data",
            false => "application/x-www-form-urlencoded",
        };
        out.set(
            "requestBody",
            J::obj([
                ("required", J::Bool(true)),
                ("content", J::obj([(kind, J::obj([("schema", schema)]))])),
            ]),
        );
    }
    if let Some(s) = security(a.bearer, a.session, used) {
        out.set("security", s);
    }
    let location = J::obj([("location", J::obj([("schema", J::of("string"))]))]);
    let mut redirect = J::obj([("description", J::str("Done: on to the page it names"))]);
    redirect.set("headers", location);
    let mut responses = vec![
        ("200".to_string(), html("The page again")),
        ("303".to_string(), redirect),
    ];
    if form {
        let what = "The page again, with what is wrong by field";
        responses.push(("422".into(), html(what)));
    }
    if a.bearer || a.session {
        responses.push(("401".into(), reference("responses", "Unauthorized")));
    }
    responses.push(("default".into(), reference("responses", "Error")));
    out.set("responses", J::Obj(responses));
    out
}

/// The struct of `types` that `t` is (through `Option`).
fn unwrapped<'a>(t: &str, types: &'a [TypeItem]) -> Option<&'a TypeItem> {
    let t = plain(t);
    (types.iter()).find(|ty| ty.name == t && ty.variants.is_empty())
}

/// The schema of a form field of type `t` with its `#[validate]` rules,
/// and whether it is a file (so the form is `multipart/form-data`).
fn form_field(
    t: &str,
    rules: Option<&(String, String)>,
    types: &[TypeItem],
    schemas: &mut Vec<(String, J)>,
) -> (J, bool) {
    let file = is_file(t);
    let mut s = match file {
        true => file_schema(t),
        false => schema(&plain(t), types, schemas),
    };
    if let Some((_, rules)) = rules {
        constrain(&mut s, rules, t);
    }
    (s, file)
}

/// `Image` or `Upload`, alone or in an `Option` or `Vec`: a file in a form.
fn is_file(t: &str) -> bool {
    let t = plain(t);
    let item = match last_segment(&t) {
        "Vec" => inner(&t).map(squeeze).unwrap_or_default(),
        _ => t,
    };
    matches!(last_segment(&item), "Image" | "Upload")
}

fn file_schema(t: &str) -> J {
    let binary = J::obj([("type", J::str("string")), ("format", J::str("binary"))]);
    match last_segment(&plain(t)) {
        "Vec" => J::obj([("type", J::str("array")), ("items", binary)]),
        _ => binary,
    }
}

/// `t` without its `Option` and any reference: a request that leaves the
/// input out gets `None`, which is not something to send.
fn plain(t: &str) -> String {
    let t = squeeze(t);
    let t = unref(&t);
    match option_inner(t) {
        Some(i) => squeeze(i),
        None => t.to_string(),
    }
}

fn operation(
    e: &Endpoint,
    op: &Op,
    short: bool,
    schemas: &mut Vec<(String, J)>,
    used: &mut Used,
) -> J {
    let mut params = Vec::new();
    let mut in_path: Vec<&str> = Vec::new();
    let mut fields = Vec::new();
    let mut required = Vec::new();
    let mut files = false;
    let mut body = None;
    for (name, t) in &op.inputs {
        // A segment this path is without.
        let at = |short| route_params(e.route, short).any(|(n, _)| n == name);
        if short && at(false) && !at(true) {
            continue;
        }
        let optional = may_omit(t);
        let file = is_file(t);
        files |= file;
        let schema = match file {
            true => file_schema(t),
            false => schema(&plain(t), e.types, schemas),
        };
        match place(e, op, name, t, short) {
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
    for (n, m) in route_params(e.route, short) {
        if !in_path.contains(&n) {
            params.push(param(n, "path", true, segment(m)));
        }
    }
    let rest = op.rest.as_ref().map(|(what, ty)| (*what, ty.as_str()));
    if let Some(("list", ty)) = rest {
        let taken: Vec<String> = op.inputs.iter().map(|(n, _)| n.clone()).collect();
        params.extend(filters(ty, e.types, &taken));
    }
    match rest.map(|r| r.0) {
        Some("list" | "get") => {
            params.push(param("if-none-match", "header", false, J::of("string")))
        }
        Some("put" | "patch" | "delete") => {
            params.push(param("if-match", "header", false, J::of("string")))
        }
        _ => {}
    }
    if op.method == "post" {
        params.push(param("idempotency-key", "header", false, J::of("string")));
    }
    let sends = !params.iter().all(|p| !is_query(p));
    let mut out = J::obj([("operationId", J::str(&operation_id(e, op, short)))]);
    if !params.is_empty() {
        out.set("parameters", J::Arr(params));
    }
    let content = |schema: J| J::obj([("schema", schema)]);
    let request = match body {
        // A POST of a `#[derive(Rest)]` type: one, or an array of them.
        Some(schema) if rest.is_some_and(|r| r.0 == "create") => {
            let many = J::obj([("type", J::str("array")), ("items", clone(&schema))]);
            let one_or_many = J::obj([("oneOf", J::Arr(vec![schema, many]))]);
            Some(J::obj([("application/json", content(one_or_many))]))
        }
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
            Some(match files {
                true => J::obj([("multipart/form-data", content(schema()))]),
                false => J::obj([
                    ("application/json", content(schema())),
                    ("application/x-www-form-urlencoded", content(schema())),
                ]),
            })
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
    if let Some(s) = security(op.bearer, op.session, used) {
        out.set("security", s);
    }
    let said = |what: &str| J::obj([("description", J::str(what))]);
    let json = |what: &str, schema: J| {
        J::obj([
            ("description", J::str(what)),
            ("content", J::obj([("application/json", content(schema))])),
        ])
    };
    let text_header = |names: &[&str]| {
        J::Obj(
            (names.iter())
                .map(|n| (n.to_string(), J::obj([("schema", J::of("string"))])))
                .collect(),
        )
    };
    let mut row = |ty: &str| schema(&format!("Row<{ty}>"), e.types, schemas);
    let mut responses: Vec<(String, J)> = match (rest, op.returns()) {
        (Some(("list", ty)), _) => {
            let rows = J::obj([("type", J::str("array")), ("items", row(ty))]);
            let mut ok = json("The rows, or lines of them for application/x-ndjson", rows);
            let mut headers = text_header(&["etag", "link"]);
            headers.set(
                "x-total-count",
                J::obj([
                    ("description", J::str("How many rows match")),
                    ("schema", J::of("integer")),
                ]),
            );
            ok.set("headers", headers);
            vec![
                ("200".into(), ok),
                ("304".into(), said("Not modified")),
                ("400".into(), reference("responses", "Error")),
            ]
        }
        (Some(("create", ty)), _) => {
            let mut ok = json("Created", row(ty));
            ok.set("headers", text_header(&["location", "etag"]));
            vec![("201".into(), ok)]
        }
        (Some(("get" | "put" | "patch", ty)), _) => {
            let mut ok = json("The row", row(ty));
            ok.set("headers", text_header(&["etag"]));
            let mut v = vec![("200".into(), ok)];
            match op.method {
                "get" => v.push(("304".into(), said("Not modified"))),
                _ => v.push(("412".into(), reference("responses", "Error"))),
            }
            v.push(("404".into(), said("Not found")));
            v
        }
        (Some(("delete", _)), _) => vec![
            ("204".into(), said("Done")),
            ("404".into(), said("Not found")),
            ("412".into(), reference("responses", "Error")),
        ],
        (_, Returns::Nothing) => vec![("204".into(), said("Done"))],
        (_, Returns::Response) => vec![("200".into(), said("A response"))],
        (_, Returns::MaybeResponse) => vec![
            ("200".into(), said("A response")),
            ("404".into(), said("Not found")),
        ],
        (_, Returns::Other) => match op.optional().as_deref() {
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
    let mut extra = |code: &str, what: J| {
        if responses.iter().all(|(k, _)| k != code) {
            responses.push((code.to_string(), what));
        }
    };
    if sends || validated {
        extra("400", reference("responses", "Error"));
    }
    if validated {
        let schema = reference("schemas", "Error");
        extra("422", json("The input did not pass", schema));
    }
    if op.bearer || op.session {
        extra("401", reference("responses", "Unauthorized"));
    }
    extra("default", reference("responses", "Error"));
    out.set("responses", J::Obj(responses));
    out
}

/// Whether parameter `p` is a query parameter.
fn is_query(p: &J) -> bool {
    let J::Obj(m) = p else { return false };
    m.iter()
        .any(|(k, v)| k == "in" && matches!(v, J::Str(s) if s == "query"))
}

/// The query parameters that filter a `#[derive(Rest)]` list: a field of
/// `ty` that is a plain value (`?done=true`, `?points.gte=3`), unless
/// `taken` names it.
fn filters(ty: &str, types: &[TypeItem], taken: &[String]) -> Vec<J> {
    let Some(item) = types.iter().find(|t| t.name == ty) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for (name, t) in &item.fields {
        let name = name.strip_prefix("r#").unwrap_or(name);
        let t = plain(t);
        if matches!(ty::scalar(&t), Scalar::Other | Scalar::Unit) || taken.iter().any(|n| n == name)
        {
            continue;
        }
        out.push(param(
            name,
            "query",
            false,
            schema(&t, types, &mut Vec::new()),
        ));
    }
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

fn place(e: &Endpoint, op: &Op, name: &str, t: &str, short: bool) -> Place {
    if name == "body" && !ty::is_maybe_text(t) {
        Place::Body
    } else if route_params(e.route, short).any(|(n, _)| n == name) {
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
fn operation_id(e: &Endpoint, op: &Op, short: bool) -> String {
    format!("{}{}", op.method, id(e.route, short))
}

/// `/notes/[id]` as an identifier's tail: `_notes_id`.
fn id(r: &Route, short: bool) -> String {
    path(r, short)
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
        "Option" => nullable(schema(arg(), types, schemas)),
        "Box" | "Arc" | "Rc" => schema(arg(), types, schemas),
        // `wisp::Email`, unless the file has one of its own.
        "Email" if types.iter().all(|x| x.name != "Email") => {
            J::obj([("type", J::str("string")), ("format", J::str("email"))])
        }
        // `wisp::Image`: its JSON is a `data:` URL.
        "Image" if types.iter().all(|x| x.name != "Image") => {
            J::obj([("type", J::str("string")), ("format", J::str("data-url"))])
        }
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

/// `schema`, or null: an `Option` that is `None` is `null` in JSON.
fn nullable(mut schema: J) -> J {
    if let J::Obj(m) = &mut schema
        && let Some((_, v)) = m.iter_mut().find(|(k, _)| k == "type")
        && let J::Str(t) = v
        && t != "null"
    {
        *v = J::Arr(vec![J::Str(std::mem::take(t)), J::str("null")]);
        return schema;
    }
    J::obj([("anyOf", J::Arr(vec![schema, J::of("null")]))])
}

/// `schema` with the limits of a field's `#[validate(…)]`: `min`, `max`,
/// `min_len`, `max_len`, `len` (a range) and `email`. A limit that is not a
/// plain number (a constant, say) is left out, not guessed.
fn constrain(schema: &mut J, rules: &str, t: &str) {
    let (min_len, max_len) = match last_segment(t) {
        "Vec" => ("minItems", "maxItems"),
        _ => ("minLength", "maxLength"),
    };
    // Written as JSON writes a number: Rust's `5.`, `1_0` or `inf` are not.
    let mut add = |key: &str, n: &str| {
        if let Some(x) = rules::plain(n) {
            schema.set(key, J::Num(x.to_string()));
        }
    };
    let mut email = false;
    for r in rules::parse(rules).unwrap_or_default().rules {
        match r.key {
            Key::Min => add("minimum", &r.value),
            Key::Max => add("maximum", &r.value),
            Key::MinLen => add(min_len, &r.value),
            Key::MaxLen => add(max_len, &r.value),
            Key::Len => {
                let (lo, hi) = r.len_bounds();
                add(min_len, &lo.map_or(String::new(), |n| n.to_string()));
                add(max_len, &hi.map_or(String::new(), |n| n.to_string()));
            }
            Key::Email => email = true,
            Key::Url | Key::OneOf | Key::Pattern | Key::With => {}
        }
    }
    if email {
        schema.set("format", J::str("email"));
    }
}

/// A struct defined in the file, field by field; an enum without fields, as
/// its names. One with no named fields (a tuple struct) is only named.
fn object(ty: &TypeItem, types: &[TypeItem], schemas: &mut Vec<(String, J)>) -> J {
    // An enum without fields is one of its variants' names.
    if !ty.variants.is_empty() {
        let names = ty.variants.iter().map(|v| J::str(v)).collect();
        return J::obj([("type", J::str("string")), ("enum", J::Arr(names))]);
    }
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
    // The path as the inside of a template literal, which a parameter
    // makes it: a folder may be named with a backtick or a `$`. Without
    // one it is the route's pattern.
    let mut tpl = String::new();
    let mut dynamic = false;
    for s in &e.route.segs {
        match s {
            Seg::Static(n) => {
                tpl.push('/');
                for c in n.chars() {
                    if matches!(c, '`' | '\\' | '$') {
                        tpl.push('\\');
                    }
                    tpl.push(c);
                }
            }
            Seg::Param(n, _) | Seg::Optional(n, _) | Seg::Rest(n) => {
                let mut a: String = n
                    .chars()
                    .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
                    .collect();
                if TS_TAKEN.contains(&a.as_str()) {
                    a.push('_');
                }
                // `[1st]`: a name cannot start with a digit.
                if a.starts_with(|c: char| c.is_ascii_digit()) {
                    a.insert(0, '_');
                }
                args.push(format!("{a}: string | number"));
                tpl.push_str(&format!("/${{encodeURIComponent({a})}}"));
                dynamic = true;
            }
        }
    }
    let path = match dynamic {
        true => format!("`{tpl}`"),
        false => q(&e.route.pattern()),
    };
    let mut fields = Vec::new();
    let mut query = Vec::new();
    let mut body = None;
    let mut query_required = false;
    for (name, t) in &op.inputs {
        match place(e, op, name, t, false) {
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
        camel(&operation_id(e, op, false)),
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
pub(crate) fn ts(t: &str, types: &[TypeItem], decls: &mut Vec<(String, String)>) -> String {
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
        "Email" | "Image" if types.iter().all(|x| x.name != last_segment(t)) => "string".into(),
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
    if !ty.variants.is_empty() {
        let names: Vec<String> = ty.variants.iter().map(|v| q(v)).collect();
        return format!("export type {} = {};\n", ty.name, names.join(" | "));
    }
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
    may_omit(t) || ty.set_by_wisp(name)
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
            page_file: String::new(),
            md: None,
            page_rs: false,
            page_js: None,
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
            &[],
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
            "\"due\":{\"type\":[\"string\",\"null\"]},\"author\":{\"description\":\"crate::User\"}",
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
            page_file: String::new(),
            md: None,
            page_rs: false,
            page_js: None,
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
        let email = text(&schema("Option<Email>", &items.types, &mut schemas));
        assert_eq!(
            email,
            "{\"type\":[\"string\",\"null\"],\"format\":\"email\"}"
        );
        assert_eq!(ts("Email", &items.types, &mut Vec::new()), "string");
        let image = text(&schema("Image", &items.types, &mut schemas));
        assert_eq!(image, "{\"type\":\"string\",\"format\":\"data-url\"}");
        assert_eq!(
            ts("Option<Image>", &items.types, &mut Vec::new()),
            "string | null"
        );
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
            "export type Kind = \"A\" | \"B\";
",
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
        // Folder names that are not JavaScript names, in a template literal.
        route.segs = vec![Seg::Static("a`$b".into()), Seg::Param("1st".into(), None)];
        let ts = typescript(&[Endpoint {
            route: &route,
            ops: &ops(&items),
            types: &items.types,
        }]);
        assert!(
            ts.contains("(_1st: string | number) => call<string>(\"GET\", `/a\\`\\$b/${encodeURIComponent(_1st)}`)"),
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
    #[validate(min = 0.5, max = 5.)]
    f: f64,
    #[validate(min = inf)]
    g: f64,
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
            // As JSON writes numbers; one that is none is left out.
            "\"f\":{\"type\":\"number\",\"minimum\":0.5,\"maximum\":5}",
            "\"g\":{\"type\":\"number\"}",
        ] {
            assert!(new.contains(want), "{want}\n{new}");
        }
    }

    fn route(segs: Vec<Seg>) -> Route {
        Route {
            segs,
            ..notes_route()
        }
    }

    #[test]
    fn enums_and_options() {
        let items = rust_scan::scan(
            "enum Kind { A, #[x] B = 2, C }\nenum Shape { Dot(u8), Line }\n\
             struct T { kind: Kind, shape: Shape, due: Option<Kind>, n: Option<u8> }",
        )
        .unwrap();
        let mut schemas = Vec::new();
        let kind = text(&schema("Kind", &items.types, &mut schemas));
        assert_eq!(kind, "{\"$ref\":\"#/components/schemas/Kind\"}");
        let kind = text(&schemas[0].1);
        assert_eq!(kind, "{\"type\":\"string\",\"enum\":[\"A\",\"B\",\"C\"]}");
        // A variant with fields is not one name: the enum is only named.
        let shape = text(&schema("Shape", &items.types, &mut schemas));
        assert!(shape.contains("Shape"), "{shape}");
        assert!(text(&schemas[1].1).contains("\"title\":\"Shape\""));
        // `None` is null.
        let opt = text(&schema("Option<u8>", &items.types, &mut schemas));
        assert_eq!(opt, "{\"type\":[\"integer\",\"null\"],\"minimum\":0}");
        let opt = text(&schema("Option<Kind>", &items.types, &mut schemas));
        assert_eq!(
            opt,
            "{\"anyOf\":[{\"$ref\":\"#/components/schemas/Kind\"},{\"type\":\"null\"}]}"
        );
        // A request that leaves it out sends no null: a query is the bare type.
        assert_eq!(plain("Option<u8>"), "u8");
    }

    #[test]
    fn pages_and_actions() {
        let items = rust_scan::scan(
            "struct Post { #[validate(len = 1..=9)] title: String, body: Option<String> }\n\
             #[action] fn default(cx: &mut Cx, post: Post) {}\n\
             #[action] fn upload(slug: String, avatar: Image, tags: Vec<String>) {}\n\
             #[action] fn mine(cx: &mut Cx) { cx.signed_in()?; }",
        )
        .unwrap();
        let actions: Vec<Action> = items.fns.iter().map(Action::of).collect();
        let r = route(vec![
            Seg::Static("p".into()),
            Seg::Param("slug".into(), None),
        ]);
        let pg = Page {
            route: &r,
            actions: &actions,
            types: &items.types,
        };
        let json = spec("app", "1", &[], &[pg]);
        for want in [
            "\"/p/{slug}\":{\"get\":{\"operationId\":\"page_p_slug\",\"tags\":[\"pages\"]",
            // The default action is the page's own address.
            "\"post\":{\"operationId\":\"action_default_p_slug\"",
            "\"required\":[\"title\"],\"properties\":{\"title\":{\"type\":\"string\",\"minLength\":1,\"maxLength\":9},\"body\":{\"type\":\"string\"}}",
            // Others are `?/name`; a file makes it multipart; the route gives `slug`.
            "\"/p/{slug}?/upload\":{\"post\":{\"operationId\":\"action_upload_p_slug\"",
            "\"multipart/form-data\":{\"schema\":{\"type\":\"object\",\"required\":[\"avatar\"],\"properties\":{\"avatar\":{\"type\":\"string\",\"format\":\"binary\"},\"tags\":{\"type\":\"array\",\"items\":{\"type\":\"string\"}}}}}",
            "{\"name\":\"slug\",\"in\":\"path\",\"required\":true,\"schema\":{\"type\":\"string\"}}",
            // A signed-in member only: the scheme is declared and named.
            "\"/p/{slug}?/mine\":{\"post\":{\"operationId\":\"action_mine_p_slug\",\"tags\":[\"actions\"],\"parameters\":[{\"name\":\"slug\"",
            "\"security\":[{\"session\":[]}]",
            "\"securitySchemes\":{\"session\":{\"type\":\"apiKey\",\"in\":\"cookie\",\"name\":\"session\"}}",
            "\"401\":{\"$ref\":\"#/components/responses/Unauthorized\"}",
        ] {
            assert!(json.contains(want), "{want}\n{json}");
        }
    }

    #[test]
    fn optional_segments_are_two_paths() {
        let items = rust_scan::scan("fn get(lang: Option<String>) -> String { todo!() }").unwrap();
        let r = route(vec![
            Seg::Optional("lang".into(), None),
            Seg::Static("about".into()),
        ]);
        let json = spec(
            "app",
            "1",
            &[Endpoint {
                route: &r,
                ops: &ops(&items),
                types: &items.types,
            }],
            &[],
        );
        assert!(json.contains("\"/{lang}/about\":{\"get\":{\"operationId\":\"get_lang_about\""));
        // Without it the path has no such parameter, and the input is not a query.
        assert!(json.contains("\"/about\":{\"get\":{\"operationId\":\"get_about\",\"responses\""));
    }

    #[test]
    fn a_resource_has_its_headers_and_keys() {
        let items = rust_scan::scan(
            "#[derive(Rest)]\n#[rest(write = \"API_KEY\", admin = \"ADMIN\")]\nstruct Note { title: String, done: bool }",
        )
        .unwrap();
        assert_eq!(items.types[0].rest.len(), 2);
        let o = |what: &'static str, method: &'static str, inputs: Vec<(&str, &str)>| Op {
            method,
            inputs: inputs
                .into_iter()
                .map(|(n, t)| (n.to_string(), t.to_string()))
                .collect(),
            value: "Row<Note>".into(),
            bearer: method != "get",
            session: false,
            rest: Some((what, "Note".into())),
        };
        let r = route(vec![Seg::Static("notes".into())]);
        let ops = [
            o("list", "get", vec![("limit", "Option<u32>")]),
            o("create", "post", vec![("body", "Note")]),
        ];
        let json = spec(
            "app",
            "1",
            &[Endpoint {
                route: &r,
                ops: &ops,
                types: &items.types,
            }],
            &[],
        );
        for want in [
            // Filters by field, once, beside the paging.
            "{\"name\":\"done\",\"in\":\"query\",\"required\":false,\"schema\":{\"type\":\"boolean\"}}",
            "{\"name\":\"if-none-match\",\"in\":\"header\"",
            "\"x-total-count\":{\"description\":\"How many rows match\"",
            // A POST creates one or several, and answers 201 with where it is.
            "\"oneOf\":[{\"$ref\":\"#/components/schemas/Note\"},{\"type\":\"array\"",
            "\"201\":{\"description\":\"Created\"",
            "\"location\":{\"schema\":{\"type\":\"string\"}}",
            "{\"name\":\"idempotency-key\",\"in\":\"header\"",
            "\"security\":[{\"bearer\":[]}]",
            "\"securitySchemes\":{\"bearer\":{\"type\":\"http\",\"scheme\":\"bearer\"}}",
        ] {
            assert!(json.contains(want), "{want}\n{json}");
        }
        assert_eq!(json.matches("\"name\":\"done\"").count(), 1);
    }

    #[test]
    fn paths_that_differ_by_a_name_are_one() {
        assert!(same_shape("/m/{n}", "/m/{slug}"));
        assert!(!same_shape("/m/{n}", "/m/{n}/x"));
        assert!(!same_shape("/m/{n}", "/m/{n}?/x"));
    }

    #[test]
    fn pretty_changes_only_the_spacing() {
        let compact = "{\"a\":[1,{\"b\":\"x, {y}: \\\"z\\\"\"}],\"c\":{},\"d\":[]}";
        let got = pretty(compact);
        assert_eq!(
            got,
            "{\n  \"a\": [\n    1,\n    {\n      \"b\": \"x, {y}: \\\"z\\\"\"\n    }\n  ],\n  \"c\": {},\n  \"d\": []\n}\n"
        );
    }
}
