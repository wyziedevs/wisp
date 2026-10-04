//! Server routes (`+server.rs`), their guards, middleware and caching, and
//! the assets a release embeds.

use super::*;

/// The CSS's hash (`None` without CSS), and the files a release build
/// embeds: (URL, file, etag).
pub(super) struct Assets {
    pub(super) css_hash: Option<String>,
    pub(super) files: Vec<(String, PathBuf, String)>,
}

/// A `+server.rs`, which serves its route, its `/[id]`, or both.
pub(super) struct ServerFile {
    pub(super) file: PathBuf,
    /// It sets `BODY_LIMIT`.
    pub(super) limit: bool,
    /// It sets `TIMEOUT`.
    pub(super) timeout: bool,
    /// It sets `CACHE` (`false`) or `CACHE_PUBLIC` (`true`).
    pub(super) cache: Option<bool>,
    /// All it serves: each of its routes takes its own handlers of it.
    pub(super) server: model::Server,
}

/// The handlers of a `+server.rs` whose route is `segs`, their shims added
/// to `shims`, and whether it has `before`. A `#[derive(Rest)]` type
/// answers each method (on the route and its `/[id]`) the file does not.
pub(super) fn server_handlers(
    items: &rust_scan::Items,
    segs: &[Seg],
    shims: &mut Vec<String>,
) -> Result<(Vec<Handler>, bool), String> {
    use crate::routes::{HANDLERS, is_member, rest_type};
    let mut handlers: Vec<Handler> = Vec::new();
    let mut before = false;
    // What `fn before` asks of every request: a token, a member.
    let mut gate = (false, false);
    for f in &items.fns {
        let at = |msg: String| format!("{}: {msg}", f.line);
        if f.action {
            return Err(at(format!(
                "`{}` is marked #[action], but actions belong in a +page.rs",
                f.name
            )));
        }
        let name = f.name.as_str();
        if name == "head" || name == "options" {
            return Err(at(format!(
                "`{name}` is never called: HEAD is answered by `get`, and OPTIONS by Wisp"
            )));
        }
        if name == "before" {
            check_before(f).map_err(at)?;
            shims.push(shim(f, Shim::Answer)?);
            before = true;
            gate = (f.bearer, f.session);
            continue;
        }
        if !HANDLERS.contains(&name) {
            continue;
        }
        let member = is_member(f, segs);
        if name == "list" && member {
            return Err(at(
                "`list` answers GET on the route; `get(id: u64)` is the one for its `/[id]`".into(),
            ));
        }
        let method = METHODS
            .iter()
            .map(|(m, _, _)| *m)
            .find(|m| *m == name || (name == "list" && *m == "get"))
            .expect("a handler name is a method or list");
        if let Some(h) = handlers
            .iter()
            .find(|h| h.member == member && h.op.method == method)
        {
            return Err(at(format!(
                "`{name}` and `{}` both answer {} on {}",
                h.shim,
                method.to_uppercase(),
                if member { "`/[id]`" } else { "the route" }
            )));
        }
        shims.push(shim(f, Shim::Endpoint)?);
        handlers.push(Handler {
            shim: f.name.clone(),
            member,
            op: Op::of(method, f),
            by_accept: false,
            sync: !f.is_async,
            empty: returns_nothing(f),
        });
    }
    if let Some(ty) = rest_type(items) {
        if segs.iter().any(
            |s| matches!(s, Seg::Param(n, _) | Seg::Optional(n, _) | Seg::Rest(n) if n == "id"),
        ) {
            return Err(format!(
                " `{ty}` is served at this route and its `/[id]`, but the route already has an `id`"
            ));
        }
        shims.extend(rest_hooks(items, ty)?);
        let one = |n: &str| vec![(n.to_string(), "u64".to_string())];
        let with_body = |mut v: Vec<(String, String)>| {
            v.push(("body".into(), ty.to_string()));
            v
        };
        let row = format!("Row<{ty}>");
        // What a list takes besides filters by field (`?done=true`).
        let paging = ["limit", "offset", "after", "sort", "fields"]
            .map(|n| {
                let t = if n == "after" {
                    "u64"
                } else if n == "limit" || n == "offset" {
                    "u32"
                } else {
                    "String"
                };
                (n.to_string(), format!("Option<{t}>"))
            })
            .to_vec();
        let auto = [
            (false, "get", "list", paging, format!("Vec<{row}>")),
            (false, "post", "create", with_body(Vec::new()), row.clone()),
            (true, "get", "get", one("id"), format!("Option<{row}>")),
            (
                true,
                "put",
                "put",
                with_body(one("id")),
                format!("Option<{row}>"),
            ),
            (
                true,
                "patch",
                "patch",
                with_body(one("id")),
                format!("Option<{row}>"),
            ),
            (true, "delete", "delete", one("id"), "Option<()>".into()),
        ];
        for (member, method, what, inputs, value) in auto {
            if handlers
                .iter()
                .any(|h| h.member == member && h.op.method == method)
            {
                continue;
            }
            let shim = format!("__rest_{what}");
            shims.push(format!(
                "pub async fn {shim}(cx: &mut ::wisp::Cx) -> ::wisp::Result<::wisp::Response> {{ ::wisp::rt::rest::{what}::<super::{ty}>(cx, &__REST_HOOKS) }}"
            ));
            // Keys of `#[rest(...)]`: a read needs `key`, a write `write` or
            // `key`, a delete `admin` too.
            let keys = items.types.iter().find(|t| t.name == ty).map(|t| &t.rest);
            let key = |k: &str| keys.is_some_and(|r| r.iter().any(|(n, _)| n == k));
            let bearer = key("key")
                || (method != "get" && key("write"))
                || (method == "delete" && key("admin"));
            let op = Op {
                method,
                inputs,
                value,
                bearer,
                session: false,
                rest: Some((what, ty.to_string())),
            };
            // A list is JSON, or NDJSON for `accept: application/x-ndjson`.
            let by_accept = what == "list";
            handlers.push(Handler {
                shim,
                member,
                op,
                by_accept,
                sync: false,
                empty: false,
            });
        }
    }
    for h in &mut handlers {
        h.op.bearer |= gate.0;
        h.op.session |= gate.1;
    }
    if handlers.is_empty() {
        return Err(
            " defines none of get, post, put, patch, delete, list, nor a `#[derive(Rest)]` type"
                .into(),
        );
    }
    Ok((handlers, before))
}

/// The hooks a `+server.rs` may define for its `#[derive(Rest)]` type, and
/// whether each is given the row (with its id) rather than the value.
pub(super) const REST_HOOKS: [(&str, bool); 6] = [
    ("before_create", false),
    ("before_update", false),
    ("before_delete", true),
    ("after_create", true),
    ("after_update", true),
    ("after_delete", true),
];

/// `__REST_HOOKS` for the `#[derive(Rest)]` type `ty`, and an adapter for
/// each hook the file defines: plain functions that take, in any order,
/// `cx`, the value (`&mut T`, or `&T`), the row (`&Row<T>`) or its `id`,
/// and return nothing or a `Result`.
pub(super) fn rest_hooks(items: &rust_scan::Items, ty: &str) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    let mut set = String::new();
    for (hook, row) in REST_HOOKS {
        let Some(f) = items.function(hook) else {
            continue;
        };
        let at = |msg: String| format!("{}: {msg}", f.line);
        let takes = match (row, hook) {
            (true, _) => format!("`cx`, `row: &Row<{ty}>`, `note: &{ty}` or `id: u64`"),
            (false, "before_update") => format!("`cx`, `note: &mut {ty}` or `id: u64`"),
            _ => format!("`cx` and `note: &mut {ty}`"),
        };
        if f.is_async || f.returns_kind() != Returns::Nothing {
            return Err(at(format!(
                "`{hook}` is a hook: a plain `fn` that returns nothing or a `Result`, taking {takes}"
            )));
        }
        let mut args = Vec::new();
        for (_, written) in &f.params {
            let t = ty::squeeze(written);
            let (shared, of) = (ty::is_shared_ref(&t), ty::unref(&t));
            let arg = match (row, t.as_str()) {
                _ if rust_scan::is_cx(&t) => "cx",
                (true, "u64") => "row.id",
                (false, "u64") if hook == "before_update" => "id",
                (true, _) if shared && of == format!("Row<{ty}>") => "row",
                (true, _) if shared && of == ty => "&row.value",
                (false, _) if t.starts_with('&') && of == ty => "v",
                _ => return Err(at(format!("`{hook}` takes {takes}, not `{t}`"))),
            };
            args.push(arg);
        }
        let call = format!("super::{hook}({})", args.join(", "));
        let back = match f.fallible {
            true => format!("{call}?; Ok(())"),
            false => format!("let () = {call}; Ok(())"),
        };
        let params = match (row, hook) {
            (true, _) => format!("row: &::wisp::Row<super::{ty}>"),
            (false, "before_update") => format!("id: u64, v: &mut super::{ty}"),
            _ => format!("v: &mut super::{ty}"),
        };
        out.push(format!(
            "#[allow(unused_variables)] fn __hook_{hook}(cx: &mut ::wisp::Cx, {params}) -> ::wisp::Result {{ {back} }}"
        ));
        set.push_str(&format!("{hook}: Some(__hook_{hook}), "));
    }
    out.push(format!(
        "static __REST_HOOKS: ::wisp::rt::rest::Hooks<super::{ty}> = ::wisp::rt::rest::Hooks {{ {set}..::wisp::rt::rest::Hooks::NONE }};"
    ));
    Ok(out)
}

/// `before`, in `src/hooks.rs` or a `+server.rs`: it takes only `cx` and
/// returns nothing, a `Result`, or a response to send instead.
pub(super) fn check_before(f: &FnItem) -> Result<(), String> {
    if f.returns_kind() == Returns::Other {
        return Err(format!(
            "`before` returns `{}`. It returns nothing (or `Result<()>`), or a `Response` to send instead \
             (or `Option<Response>`, to send one only sometimes).",
            f.returns
        ));
    }
    if f.params.iter().any(|(_, ty)| !rust_scan::is_cx(ty)) {
        return Err("`before` takes only `cx`: `fn before(cx: &mut Cx)`".into());
    }
    Ok(())
}

/// `+server.rs` function, the `Method` variants it serves, its `Allow` entry.
pub(super) const METHODS: [(&str, &str, &str); 5] = [
    ("get", "Get | Head", "GET, HEAD"),
    ("post", "Post", "POST"),
    ("put", "Put", "PUT"),
    ("patch", "Patch", "PATCH"),
    ("delete", "Delete", "DELETE"),
];

/// Whether the path `url` (`/a/b.png`, encoded) may be one the route arm
/// `exp` matches: by its segments, a parameter any but an empty one (a
/// matcher's, any), a `[...rest]` anything.
/// The literal `href="/…"` values in `html`, a template's text.
pub(super) fn hrefs(html: &str) -> impl Iterator<Item = &str> {
    (html.split("href=\"").skip(1))
        .filter_map(|s| s.split_once('"').map(|(v, _)| v))
        .filter(|v| v.starts_with('/'))
}

pub(super) fn may_match(exp: &[&Seg], url: &str) -> bool {
    if exp.iter().any(|s| matches!(s, Seg::Rest(_))) {
        return true;
    }
    let segs: Vec<&str> = url.trim_start_matches('/').split('/').collect();
    segs.len() == exp.len()
        && exp.iter().zip(&segs).all(|(e, s)| match e {
            Seg::Static(x) => encode_path(x) == *s,
            _ => !s.is_empty(),
        })
}

/// The method `h` answers: its name, its `Method` variants, and its `Allow` names.
pub(super) fn method_of(h: &Handler) -> &'static (&'static str, &'static str, &'static str) {
    METHODS.iter().find(|(n, _, _)| *n == h.op.method).unwrap()
}

/// The arms of route `r` that `App::handle_now` answers without a future:
/// the `+server.rs` handlers with a sync twin (`Handler::sync`), on a route
/// that never waits, with no `before` hook of the app's or the file's.
pub(super) fn now_arms<'a>(
    p: &Project,
    r: &'a model::Route,
) -> Vec<(&'a model::Server, &'a Handler)> {
    let now = !p.has_hook("before") && !p.model.route_waits(r);
    (r.server.iter().filter(|s| now && !s.before))
        .flat_map(|s| s.handlers.iter().filter(|h| h.sync).map(move |h| (s, h)))
        .collect()
}

/// A file's `const RATE_LIMIT: u32 = 60;` (requests a minute per client
/// address) and `const CORS: &str = "*";`, checked: the statements that
/// enforce them, the first thing its requests run, `RateLimit` in `shims`.
pub(super) fn guards(
    root: Option<&Path>,
    items: &rust_scan::Items,
    rel: &str,
    shims: &mut Vec<String>,
) -> Result<String, String> {
    let mut out = String::new();
    let get = |name: &str, ok: fn(&str) -> bool, want: &str| {
        let Some(c) = items.constant(name) else {
            return Ok(false);
        };
        match ok(&c.ty) && !c.is_static {
            true => Ok(true),
            false => Err(format!(
                "{rel}:{}: `{name}` is a `{}`; make it {want}",
                c.line, c.ty
            )),
        }
    };
    let str_ = |t: &str| t.starts_with('&') && t.ends_with("str");
    if get("CORS", str_, "a `&str`: `const CORS: &str = \"*\";`")? {
        out.push_str("cx.cors(super::CORS)?; ");
    }
    let rate = "a `u32`, the requests a minute per client: `const RATE_LIMIT: u32 = 60;`";
    if get("RATE_LIMIT", |t| t == "u32", rate)? {
        shims.push(
            "pub static __RATE: ::wisp::RateLimit = ::wisp::RateLimit::per_minute(super::RATE_LIMIT);"
                .into(),
        );
        out.push_str("__RATE.check(cx.client_ip())?; ");
    }
    out.push_str(&middleware(root, items, rel)?);
    Ok(out)
}

/// A file's `const MIDDLEWARE: &[&str] = &["auth", "audit"];`, checked
/// against `src/middleware.rs` (a `pub fn auth(cx: &mut Cx) -> Result` each):
/// the calls, in order, before the request does anything else. A route that
/// names none runs none.
pub(super) fn middleware(
    root: Option<&Path>,
    items: &rust_scan::Items,
    rel: &str,
) -> Result<String, String> {
    let Some(c) = items.constant("MIDDLEWARE") else {
        return Ok(String::new());
    };
    let shape =
        "`const MIDDLEWARE: &[&str] = &[\"auth\"];`, names of functions in src/middleware.rs";
    let at = |why: String| format!("{rel}:{}: {why}", c.line);
    let Some(root) = root else {
        return Err(at(
            "hooks.rs has `before`, which runs for every request: call the functions there".into(),
        ));
    };
    let list = (c.value.trim().strip_prefix('&'))
        .and_then(|v| v.trim().strip_prefix('[')?.strip_suffix(']'));
    let list = list.filter(|_| !c.is_static);
    let mut names = Vec::new();
    for n in list
        .ok_or_else(|| at(format!("`MIDDLEWARE` must be {shape}")))?
        .split(',')
    {
        let n = n.trim();
        if n.is_empty() {
            continue;
        }
        let n = n.trim_matches('"');
        let ident = !n.is_empty()
            && !n.starts_with(|c: char| c.is_ascii_digit())
            && n.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_');
        if !ident {
            return Err(at(format!(
                "`{n}` is not a function name; `MIDDLEWARE` is {shape}"
            )));
        }
        names.push(n);
    }
    let file = root.join("src").join("middleware.rs");
    let src = crate::read_source(&file).map_err(|_| at(format!("`MIDDLEWARE` names functions of src/middleware.rs, which is not there: add it with `pub fn {}(cx: &mut Cx) -> Result`", names.first().copied().unwrap_or("auth"))))?;
    let known = rust_scan::scan(&src).map_err(|e| format!("src/middleware.rs: {e}"))?;
    // The const is read, so it is not unused.
    let mut out = String::from("let _ = super::MIDDLEWARE; ");
    for n in names {
        if !known.function(n).is_some_and(|f| f.public) {
            return Err(at(format!(
                "src/middleware.rs has no `pub fn {n}(cx: &mut Cx) -> Result`"
            )));
        }
        out.push_str(&format!("middleware::{n}(cx)?; "));
    }
    Ok(out)
}

/// A file's `const TIMEOUT: u32 = 5;` (seconds, then a 503), checked.
pub(super) fn timeout(
    items: &rust_scan::Items,
    rel: &str,
    shims: &mut Vec<String>,
) -> Result<bool, String> {
    let Some(c) = items.constant("TIMEOUT") else {
        return Ok(false);
    };
    if c.ty != "u32" || c.is_static {
        return Err(format!(
            "{rel}:{}: `TIMEOUT` is a `{}`; make it a `u32` of seconds: `const TIMEOUT: u32 = 5;`",
            c.line, c.ty
        ));
    }
    shims.push("pub const TIMEOUT: u32 = super::TIMEOUT;".into());
    Ok(true)
}

/// Makes `guard` run first in the file's `before`, which it adds when the
/// file has none (`has`). Whether it added one.
pub(super) fn add_guard(shims: &mut Vec<String>, guard: &str, has: bool) -> bool {
    const OPEN: &str = "-> ::wisp::Result<Option<::wisp::Response>> { ";
    if guard.is_empty() {
        return false;
    }
    match shims
        .iter_mut()
        .find(|s| s.starts_with("pub async fn before("))
    {
        Some(s) => *s = s.replacen(OPEN, &format!("{OPEN}{guard}"), 1),
        None => shims.push(format!(
            "pub async fn before(cx: &mut ::wisp::Cx) {OPEN}{guard}Ok(None) }}"
        )),
    }
    !has
}

/// What goes around an arm of `r` served by `module`: its `TIMEOUT`, if that
/// module sets it.
pub(super) fn within(r: &model::Route, module: &str) -> (String, &'static str) {
    match &r.timeout {
        Some(m) if m == module => (
            format!("::wisp::rt::within({m}::__call::TIMEOUT, async "),
            ").await",
        ),
        _ => (String::new(), ""),
    }
}

/// The statement in `handle` that runs an `Answer` shim (an action or
/// `before`): a `Response` it hands back is sent instead of the page.
pub(super) fn answer(shim: &str) -> String {
    format!("if let Some(r) = {shim}(cx).await? {{ ::wisp::rt::respond(__o, r); return Ok(()); }}")
}

/// The shims of a route's `CACHE` (or `CACHE_PUBLIC`, named `name`): its
/// seconds, and what `CACHE_STALE` and `CACHE_TAGS` add, if the module sets
/// them.
pub(super) fn cache_shim(items: &rust_scan::Items, name: &str) -> String {
    let stale = items
        .constant("CACHE_STALE")
        .map_or("0", |_| "super::CACHE_STALE");
    let tags = items
        .constant("CACHE_TAGS")
        .map_or("&[]", |_| "super::CACHE_TAGS");
    format!(
        "pub const CACHE: u32 = super::{name};
pub const MORE: ::wisp::rt::CacheMore = ::wisp::rt::CacheMore::new({stale}, {tags});"
    )
}

/// Sets a route's `BODY_LIMIT` or `CACHE`, which its page (in file
/// `page`) and the `+server.rs` beside it cannot both set.
pub(super) fn set_once<T>(
    slot: &mut Option<T>,
    v: T,
    name: &str,
    page: &str,
) -> Result<(), String> {
    if slot.is_some() {
        return Err(format!(
            "`{name}` is also set in this route's {page}; set it in one place"
        ));
    }
    *slot = Some(v);
    Ok(())
}

/// Whether a file under `dir` (Rust or `.wisp`; not `target`) says `trailing_slash`, or
/// cannot be read: the app may then set how a page's address ends.
pub(super) fn mentions_slash(dir: &Path) -> bool {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return true;
    };
    entries.flatten().any(|e| {
        let path = e.path();
        if path.is_dir() {
            let name = e.file_name();
            let name = name.to_string_lossy();
            return !(name == "target" || name == "node_modules" || name.starts_with('.'))
                && mentions_slash(&path);
        }
        let ext = path.extension().and_then(|x| x.to_str());
        matches!(ext, Some("rs" | "wisp"))
            && std::fs::read_to_string(&path).map_or(true, |s| s.contains("trailing_slash"))
    })
}
