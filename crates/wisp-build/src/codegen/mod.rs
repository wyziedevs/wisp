//! Emits `$OUT_DIR/wisp.rs`: user modules, one render function per template,
//! the router `match`, and the `wisp::App` impl.
//!
//! The output is meant to be read. When rustc reports an error inside a
//! template expression, the offending line ends with `// file.wisp:line`.
//!
//! A template with browser code (a client script, directives) also gets an
//! ES module, built here as text and compiled in: see `client`.

use crate::js::Kind as JsKind;
use crate::json_str as js_str;
use crate::model::{self, Handler, Model};
use crate::npm::{self, Npm};
use crate::openapi::{self, Op};
use crate::protocol::{
    APP_CSS_PATH, AWAIT_CLOSE, AWAIT_JS, COPY_END, COPY_START, ELEMENT_JS_PATH, ELEMENTS,
    EXTRA_JS_PATH, GROUP_ATTR, IMAGES, ISLAND_MEDIA, LIVE_JS_PATH, LOOP_ATTR, MODULES, NPM_MODULES,
    ON_FLAGS, ON_PLACED, ON_ROOT, REMOTE, REMOTE_JS_PATH, SLOT_ATTR, WISP_JS_PATH,
};
use crate::routes::Seg;
use crate::rust_scan::{self, FnItem, Returns};
use crate::template::{self, Code, Dir, Directive, Node, PropDecl, PropValue, Template};
use crate::{fnv1a, fold, i18n, image, js, rules, shell, sourcemap, ty};
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
/// The runtime's less used half: served when a module uses it.
use wisp_shared::EXTRA_JS;

mod app;
mod client;
mod hooks;
mod paint;
mod project;
mod render;
mod route;
mod server;
#[cfg(test)]
mod tests;

use client::*;
use hooks::*;
use paint::*;
use project::*;
use render::*;
use route::*;
use server::*;

pub struct Input<'a> {
    pub root: &'a Path,
    pub release: bool,
    /// Source maps for browser modules: in dev, and in release with
    /// `wisp build --sourcemap`.
    pub maps: bool,
    /// The pages `wisp build` prerendered (`WISP_PRERENDERED`): a folder
    /// of `N.html` files and `index.tsv`, a line per file: its route's
    /// pattern, its path and its name (see `wisp::export::prerender`).
    pub prerendered: Option<&'a Path>,
}

/// What a template is for; decides its render signature.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Page,
    Layout,
    Error,
    Component,
}

/// A component: `src/components/Card.wisp` is `<Card>`.
pub struct Comp {
    /// Its file, from the project root: `src/components/Card.wisp`.
    pub rel: String,
    pub name: String,
    pub module: String,
    pub props: Vec<PropDecl>,
    /// Shows its children: `{@render children()}`.
    pub children: bool,
    /// The props a parent may `bind:`: those its script's `$props()`
    /// marks `$bindable`, or any, without one.
    pub bindable: Option<Vec<String>>,
    /// Takes any prop (its `$props()` has `...rest`): the ones it does not
    /// name go in its `__rest`.
    pub rest: bool,
    /// Has browser code, so `client:visible` and the like have a module
    /// to load late.
    pub live: bool,
}

struct Tpl {
    id: usize,
    module: String,
    rel: String,
    kind: Kind,
    /// User module whose items the template can see, and whether it has
    /// `load`. The template's module goes inside it, so it sees private
    /// items and fields too.
    user: Option<(String, bool)>,
    /// The fields of its `Data`, which the template reads by name.
    data: Vec<(String, String)>,
    /// A page's `+page.js`.
    load_js: Option<PathBuf>,
    /// The statements of its `---` block (as long as the file, the rest
    /// blanked), which run before it renders and whose names it reads; and
    /// a `let` per route parameter they or the markup name.
    stmts: Option<(String, Vec<String>)>,
    t: Template,
    /// Its markup calls `t("key")`, which reads the request's locale.
    i18n: bool,
    /// A layout's slots (`@name` folders): each a parameter `name` of its
    /// render, which `{@render name()}` calls.
    slots: Vec<String>,
}

impl Tpl {
    /// Its module's path from the generated file's top.
    fn path(&self) -> String {
        match &self.user {
            Some((m, _)) => format!("{m}::{}", self.module),
            None => self.module.clone(),
        }
    }
}

/// A Rust file of the app's own that Wisp includes: `src/hooks.rs`, a
/// `+layout.rs`, `+page.rs` or `+server.rs`, a param matcher. It becomes a
/// module with the prelude in scope, and a `__call` module inside it where
/// the generated code calls its functions (so they need not be `pub`).
struct UserMod {
    name: String,
    file: PathBuf,
    /// The items of a `.wisp` file's `---` block, which stand for the file
    /// (as long as it, the rest blanked). `None`: the file is Rust.
    inline: Option<String>,
    /// The items of `__call`.
    shims: Vec<String>,
    /// The tables it keeps (`super::TODOS`), which load at startup.
    tables: Vec<String>,
    /// Its `#[derive(Config)]` types, which read the environment before `init`.
    configs: Vec<String>,
}

impl UserMod {
    fn new(
        name: String,
        file: PathBuf,
        inline: Option<String>,
        shims: Vec<String>,
        items: &rust_scan::Items,
    ) -> UserMod {
        UserMod {
            name,
            file,
            inline,
            shims,
            tables: items.tables(),
            configs: items.configs(),
        }
    }

    /// Its `__call` module: the shims, and `__ready`, which loads its
    /// tables.
    fn calls(&self) -> Vec<String> {
        let mut out = self.shims.clone();
        if !self.configs.is_empty() {
            let each: String = (self.configs.iter())
                .map(|c| format!("{c}::load()?; "))
                .collect();
            out.push(format!(
                "pub fn __config() -> ::wisp::Result<()> {{ {each}Ok(()) }}"
            ));
        }
        if !self.tables.is_empty() {
            let each: String = self
                .tables
                .iter()
                .map(|t| format!("{t}.ready(); "))
                .collect();
            out.push(format!("pub fn __ready() {{ {each}}}"));
        }
        out
    }

    /// Whether it has a shim for the function `name`.
    fn has(&self, name: &str) -> bool {
        self.shims.iter().any(|s| {
            s.starts_with(&format!("pub async fn {name}("))
                || s.starts_with(&format!("pub fn {name}("))
        })
    }
}

/// A page's or layout's Rust, from whichever place it is in.
struct Logic {
    items: rust_scan::Items,
    /// Its `+page.rs` or `+layout.rs`, or the `.wisp` whose `---` block it
    /// is; `None` when it has none.
    file: Option<PathBuf>,
    /// The block's items, for `UserMod::inline`.
    inline: Option<String>,
    /// The block's statements.
    stmts: Option<String>,
}

/// Whether `ident` is in `code` as a name of its own.
fn names(code: &str, ident: &str) -> bool {
    let word = |c: char| c.is_alphanumeric() || c == '_';
    code.match_indices(ident)
        .any(|(i, _)| !code[..i].ends_with(word) && !code[i + ident.len()..].starts_with(word))
}

/// A `#[remote]` function: browser code's `await name(args)`.
struct RemoteFn {
    f: FnItem,
    /// Its module from the generated file's top: `page_3`, `__mods::remote`.
    module: String,
    /// Its file, from the project root.
    rel: String,
    /// The types its signature may name, for its TypeScript.
    types: Vec<rust_scan::TypeItem>,
}

impl RemoteFn {
    /// Where it is served: `/_app/r/` and a hash of its file and name.
    fn path(&self) -> String {
        let id = format!("{}\0{}", self.rel, self.f.name);
        format!("{REMOTE}{:016x}", fnv1a(id.as_bytes()))
    }

    fn get(&self) -> bool {
        self.f.remote == Some(rust_scan::Remote::Get)
    }
}

/// What the generated code expects back from a function it calls.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Shim {
    /// `load`: its `Data`.
    Load,
    /// An action or `before`: a `Response` to send instead of the page, if any.
    Answer,
    /// A `+server.rs` method: the response (JSON for a value).
    Endpoint,
    /// `init`: nothing.
    Init,
}

/// The `__call` function for `f`: reads its inputs (the parameters other
/// than `cx`) from the request by name, calls it, and hands back what
/// `kind` says, whatever `f` returns. How each input is read, and what a
/// value answers, is rustc's to pick by type (`wisp::rt_traits`); only a
/// borrow (`&str`), which needs an owner, and `body` are told here. The
/// file's errors are the caller's to prefix with its path.
fn shim(f: &FnItem, kind: Shim) -> Result<String, String> {
    if !f.checks.is_empty() && !(kind == Shim::Answer && f.action) {
        return Err(format!(
            "{}: `#[validate]` on a parameter checks an action's input; an endpoint's `body: T` is checked by `T`'s fields",
            f.line
        ));
    }
    let mut args = Vec::new();
    if f.implicit_cx {
        args.push("cx".to_string());
    }
    // Each input as (its name in the call, what reads it, its owned type
    // when the call borrows it).
    let mut read = Vec::new();
    let mut inputs = f.inputs()?.into_iter();
    for (_, ty) in &f.params {
        if rust_scan::is_cx(ty) {
            args.push("cx".to_string());
            continue;
        }
        let (name, ty) = inputs.next().expect("an input per parameter but cx");
        let v = format!("__a{}", args.len());
        let get = format!("::wisp::rt_traits::FromInput::get(cx, {})", lit(name));
        let (get, owned, arg) = if name == "body" && !ty::is_maybe_text(ty) {
            // `body: T` is the whole JSON body; a string `body` is still
            // a field of that name.
            ("::wisp::rt::input::body(cx)".to_string(), "", v.clone())
        } else if ty::is_str_ref(ty) {
            (get, "String", format!("&{v}"))
        } else if ty::option_inner(ty).is_some_and(ty::is_str_ref) {
            (get, "Option<String>", format!("{v}.as_deref()"))
        } else {
            (get, "", v.clone())
        };
        read.push((name, v, get, owned));
        args.push(arg);
    }
    if let Some((p, _)) = (f.checks.iter()).find(|(p, _)| !read.iter().any(|(n, ..)| n == p)) {
        return Err(format!(
            "{}: `#[validate]` is on `{p}`, which is not read from the request",
            f.line
        ));
    }
    let mut lets = String::new();
    let nothing_to_check = f.checks.is_empty()
        && !(f.params.iter()).any(|(p, _)| unsized_upload(f, p) || unruled_password(f, p));
    if let ([(_, v, get, owned)], true) = (read.as_slice(), nothing_to_check) {
        // One input, nothing to check: its error is the answer.
        let ty = annotation(owned, "{}");
        lets = format!("let {v}{ty} = {get}?; ");
    } else if !read.is_empty() {
        // Each input is read, and checked, before any answer: every one
        // that does not pass is listed in one 422.
        lets.push_str("let mut __p = ::wisp::json::Problems::default(); ");
        for (name, v, get, owned) in &read {
            let ty = annotation(owned, "Option<{}>");
            lets.push_str(&format!(
                "let {v}{ty} = ::wisp::rt::input::read(&mut __p, {get})?; "
            ));
            for (p, rules) in &f.checks {
                if p == name {
                    lets.push_str(&checks(name, v, rules).map_err(|e| format!("{}: {e}", f.line))?);
                }
            }
            if unsized_upload(f, name) {
                lets.push_str(
                    &checks(name, v, DEFAULT_SIZE).map_err(|e| format!("{}: {e}", f.line))?,
                );
            }
            if unruled_password(f, name) {
                lets.push_str(&checks(name, v, &password_min())?);
            }
        }
        let names: Vec<&str> = read.iter().map(|(_, v, ..)| v.as_str()).collect();
        let some: String = names.iter().map(|v| format!("Some({v}), ")).collect();
        lets.push_str(&format!(
            "let ({some}true) = ({}, __p.is_empty()) else {{ return ::wisp::rt::input::refused(__p); }}; ",
            names.join(", ")
        ));
    }
    let call = format!(
        "super::{}({}){}{}",
        f.name,
        args.join(", "),
        if f.is_async { ".await" } else { "" },
        if f.fallible { "?" } else { "" }
    );
    let name = &f.name;
    Ok(match kind {
        // `Data` may be private to the file, and a value of a private type
        // cannot leave its module: it goes out in a public box that only
        // the file's module (and its template, inside it) can open.
        Shim::Load => format!(
            "pub struct Loaded(pub(super) super::Data); \
             pub async fn load(cx: &mut ::wisp::Cx) -> ::wisp::Result<Loaded> {{ {lets}Ok(Loaded({call})) }}"
        ),
        Shim::Answer => format!(
            "pub async fn {name}(cx: &mut ::wisp::Cx) -> ::wisp::Result<Option<::wisp::Response>> {{ \
             {lets}Ok(::wisp::rt_traits::Answer::answer({call})) }}"
        ),
        // Most particular first: see `wisp::rt_traits::ret`.
        // A sync one has a sync twin, `{name}_now`, for `App::handle_now`,
        // on a line of its own: `call_mod` leaves out those it does not use.
        // One that returns nothing answers nothing: the arm sends a 204.
        Shim::Endpoint => {
            let (body, ret) = match returns_nothing(f) {
                true => (format!("{lets}{call}; Ok(()) }}"), "()"),
                false => (
                    format!(
                        "use ::wisp::rt_traits::ret::*; {lets}(&&&Ret::new({call})).respond() }}"
                    ),
                    "::wisp::Response",
                ),
            };
            let now = match f.is_async {
                true => String::new(),
                false => format!(
                    "\npub fn {name}_now(cx: &mut ::wisp::Cx) -> ::wisp::Result<{ret}> {{ {body}"
                ),
            };
            format!(
                "pub async fn {name}(cx: &mut ::wisp::Cx) -> ::wisp::Result<{ret}> {{ {body}{now}"
            )
        }
        Shim::Init => {
            format!("pub async fn init() -> ::wisp::Result<()> {{ let () = {call}; Ok(()) }}")
        }
    })
}

/// The `__call` function of `#[remote] fn f`, `__r_f`: reads each argument
/// by name from the JSON object a POST sends (a GET's query), as
/// `FromJson`, checks every one before any answer (one 422 lists each that
/// does not pass), calls it, and answers what it returns as an endpoint's
/// value is. `line: msg` errors.
fn remote_shim(f: &FnItem) -> Result<String, String> {
    let mut args = Vec::new();
    if f.implicit_cx {
        args.push("cx".to_string());
    }
    let mut lets = String::from(
        "let __v = ::wisp::rt::remote::args(cx)?; let __m = ::wisp::rt::remote::members(&__v); \
         let mut __p = ::wisp::json::Problems::default(); ",
    );
    let mut names = Vec::new();
    let mut inputs = f.inputs()?.into_iter();
    for (_, ty) in &f.params {
        if rust_scan::is_cx(ty) {
            args.push("cx".to_string());
            continue;
        }
        let (name, ty) = inputs.next().expect("an input per parameter but cx");
        let v = format!("__a{}", names.len());
        // A borrow (`&str`) is read owned and lent.
        let (owned, arg) = if ty::is_str_ref(ty) {
            ("String".to_string(), format!("&{v}"))
        } else if ty::option_inner(ty).is_some_and(ty::is_str_ref) {
            ("Option<String>".to_string(), format!("{v}.as_deref()"))
        } else {
            (ty.to_string(), v.clone())
        };
        let _ = write!(
            lets,
            "let {v}: Option<{owned}> = __p.field(__m, {}); ",
            lit(name)
        );
        for (p, rules) in &f.checks {
            if p == name {
                lets.push_str(&checks(name, &v, rules).map_err(|e| format!("{}: {e}", f.line))?);
            }
        }
        if unruled_password(f, name) {
            lets.push_str(&checks(name, &v, &password_min())?);
        }
        names.push(v);
        args.push(arg);
    }
    if let Some((p, _)) = (f.checks.iter()).find(|(p, _)| !f.params.iter().any(|(n, _)| n == p)) {
        return Err(format!(
            "{}: `#[validate]` is on `{p}`, which is not an argument",
            f.line
        ));
    }
    if names.is_empty() {
        // Nothing to read: what was sent is not looked at.
        lets.clear();
    } else {
        let some: String = names.iter().map(|v| format!("Some({v}), ")).collect();
        let _ = write!(
            lets,
            "let ({some}true) = ({}, __p.is_empty()) else {{ return ::wisp::rt::input::refused(__p); }}; ",
            names.join(", ")
        );
    }
    let call = format!(
        "super::{}({}){}{}",
        f.name,
        args.join(", "),
        if f.is_async { ".await" } else { "" },
        if f.fallible { "?" } else { "" }
    );
    let body = match returns_nothing(f) {
        true => format!("{call}; Ok(::wisp::Response::empty(204))"),
        false => format!("use ::wisp::rt_traits::ret::*; (&&&Ret::new({call})).respond()"),
    };
    Ok(format!(
        "pub async fn __r_{}(cx: &mut ::wisp::Cx) -> ::wisp::Result<::wisp::Response> {{ {lets}{body} }}",
        f.name
    ))
}

/// Whether `f` returns nothing (no `->`, or `-> ()`): its answer is a 204.
fn returns_nothing(f: &FnItem) -> bool {
    f.returns.is_empty() || f.returns == "()"
}

/// What an arm of `handle` does to answer with handler `h` of module `m`,
/// its shim called as `call` (`get(cx).await`, `get_now(cx)`).
fn serve(m: &str, h: &Handler, call: &str) -> String {
    match h.empty {
        true => format!("{m}::__call::{call}?; ::wisp::rt::no_content(__o);"),
        false => format!("::wisp::rt::respond(__o, {m}::__call::{call}?);"),
    }
}

/// `: T` on the `let` of an input read as `owned` (in `how`: `{}`, or
/// `Option<{}>`) when the call borrows it; nothing when it takes it as read.
fn annotation(owned: &str, how: &str) -> String {
    match owned {
        "" => String::new(),
        t => format!(": {}", how.replace("{}", t)),
    }
}

/// The code a shim runs after reading the input `name` into `v` (an
/// `Option`, `None` when it did not pass), for its `#[validate(rules)]`:
/// the first rule that fails is its problem, and the page shows again as a
/// 422 with it.
fn checks(name: &str, v: &str, rules: &str) -> Result<String, String> {
    let each: String = (rules::parse(rules)?.checks("__v").iter())
        .map(|c| format!("__p.check({}, {c}); ", lit(name)))
        .collect();
    Ok(format!("if let Some(__v) = &{v} {{ {each}}} "))
}

/// What an upload with no `max_size` is held to: `::wisp::MAX_SIZE`.
const DEFAULT_SIZE: &str = "max_size = ::wisp::MAX_SIZE";

/// What a `Password` with no least length of its own is held to.
fn password_min() -> String {
    format!("min_len = {}", rules::PASSWORD_MIN_LEN)
}

/// Whether parameter `p` of `f` is a `Password` with no `min_len` or `len`
/// of its own.
fn unruled_password(f: &FnItem, p: &str) -> bool {
    let password = f
        .params
        .iter()
        .any(|(n, t)| n == p && rules::is_password(t));
    password
        && !(f.checks.iter())
            .any(|(c, r)| c == p && rules::parse(r).is_ok_and(|v| rules::sets_min_len(&v.rules)))
}

/// Whether parameter `p` of `f` is an upload with no `max_size` of its own.
fn unsized_upload(f: &FnItem, p: &str) -> bool {
    let upload = f.params.iter().any(|(n, t)| n == p && rules::is_upload(t));
    upload
        && !(f.checks.iter())
            .any(|(c, r)| c == p && rules::parse(r).is_ok_and(|v| v.max_size.is_some()))
}

/// The bytes the uploads of a page's actions may take in all, from their
/// `#[validate(max_size = …)]`, as code (`0 + (1 * MB)`), which the route's
/// body limit makes room for; `None` when none says. `max_size` on a
/// parameter that is not an `Image` is an error: `line: msg`.
fn upload_sizes(fns: &[FnItem]) -> Result<Option<String>, String> {
    let mut sum = String::new();
    for f in fns.iter().filter(|f| f.action) {
        for (p, rules) in &f.checks {
            let ty = (f.params.iter())
                .find(|(n, _)| n == p)
                .map_or("", |(_, t)| t.as_str());
            let rules = rules::validate(rules, p, ty).map_err(|e| format!("{}: {e}", f.line))?;
            if let Some(size) = rules.max_size {
                let _ = write!(sum, " + ({size}) as usize");
            }
        }
        for _ in f.params.iter().filter(|(p, _)| unsized_upload(f, p)) {
            sum.push_str(" + ::wisp::MAX_SIZE");
        }
    }
    Ok((!sum.is_empty()).then(|| format!("0{sum}")))
}

/// `{#each db::items().await as item}` in a page: each markup expression
/// that awaits, outside any block, becomes a statement that runs before the
/// page renders (`let __wisp_a0 = db::items().await;`), in order, and the
/// markup reads its name. Returned as the statement and its line. One inside
/// a block, which may not run, is an error: `line: msg`.
fn hoist_awaits(nodes: &mut [Node], out: &mut Vec<(String, u32)>) -> Result<(), String> {
    fn take(code: &mut Code, out: &mut Vec<(String, u32)>) {
        if rust_scan::awaits(&code.src) {
            let name = format!("__wisp_a{}", out.len());
            out.push((format!("let {name} = {};", code.src), code.line));
            code.src = name;
        }
    }
    let inside = |line: u32| {
        format!(
            "{line}: `.await` in markup runs before the page renders, so it goes outside any block \
             (`{{#each db::items().await as item}}`); inside one, await in the `---` block and name the value"
        )
    };
    for n in nodes {
        // What stays of it is inside a block.
        let rest: &[Node] = match n {
            Node::Expr(code)
            | Node::Html(code)
            | Node::Attr { code, .. }
            | Node::Bool { code, .. } => {
                take(code, out);
                &[]
            }
            Node::Each { iter, .. } => {
                take(iter, out);
                std::slice::from_ref(n)
            }
            Node::If { branches, .. } => {
                take(&mut branches[0].0, out);
                std::slice::from_ref(n)
            }
            Node::Match { scrutinee, .. } => {
                take(scrutinee, out);
                std::slice::from_ref(n)
            }
            Node::Head(children) => {
                hoist_awaits(children, out)?;
                &[]
            }
            _ => std::slice::from_ref(n),
        };
        if let Some(line) = first_await(rest) {
            return Err(inside(line));
        }
    }
    Ok(())
}

/// Calls `f` with each Rust expression of `nodes`' (not `{:case}`
/// patterns), inside blocks and children too.
fn for_each_code(
    nodes: &mut [Node],
    f: &mut dyn FnMut(&mut Code) -> Result<(), String>,
) -> Result<(), String> {
    fn all<'a>(
        bs: impl Iterator<Item = &'a mut Vec<Node>>,
        f: &mut dyn FnMut(&mut Code) -> Result<(), String>,
    ) -> Result<(), String> {
        bs.into_iter().try_for_each(|b| for_each_code(b, f))
    }
    for n in nodes {
        match n {
            Node::Expr(c) | Node::Html(c) | Node::Const(c) | Node::Selected(c) => f(c)?,
            Node::Attr { code, .. } | Node::Bool { code, .. } => f(code)?,
            Node::RenderSnippet { args, .. } => f(args)?,
            Node::Snippet { body, .. } | Node::Head(body) => for_each_code(body, f)?,
            Node::Kept { sent, own, .. } => {
                for_each_code(sent, f)?;
                all(own.iter_mut(), f)?;
            }
            Node::Chosen {
                own: Some(own),
                line,
                ..
            } => {
                let mut c = Code {
                    src: std::mem::take(own),
                    line: *line,
                };
                let r = f(&mut c);
                *own = c.src;
                r?;
            }
            Node::If {
                branches,
                otherwise,
            } => {
                for (c, b) in branches {
                    f(c)?;
                    for_each_code(b, f)?;
                }
                all(otherwise.iter_mut(), f)?;
            }
            Node::Each {
                iter,
                body,
                otherwise,
                ..
            } => {
                f(iter)?;
                for_each_code(body, f)?;
                all(otherwise.iter_mut(), f)?;
            }
            Node::Match { scrutinee, arms } => {
                f(scrutinee)?;
                all(arms.iter_mut().map(|(_, b)| b), f)?;
            }
            Node::Await {
                future,
                pending,
                then,
                catch,
            } => {
                f(future)?;
                for_each_code(pending, f)?;
                all(then.iter_mut().chain(catch).map(|(_, b)| b), f)?;
            }
            Node::Component {
                props, children, ..
            } => {
                for p in props {
                    if let PropValue::Expr(c) = &mut p.value {
                        f(c)?;
                    }
                }
                all(children.iter_mut(), f)?;
            }
            Node::Client(branches) => all(branches.iter_mut().map(|(_, b)| b), f)?,
            _ => {}
        }
    }
    Ok(())
}

/// The line of the first markup expression in `nodes` that awaits.
fn first_await(nodes: &[Node]) -> Option<u32> {
    let code = |c: &Code| rust_scan::awaits(&c.src).then_some(c.line);
    fn all<'a>(mut bs: impl Iterator<Item = &'a Vec<Node>>) -> Option<u32> {
        bs.find_map(|b| first_await(b))
    }
    nodes.iter().find_map(|n| match n {
        Node::Expr(c) | Node::Html(c) | Node::Const(c) => code(c),
        Node::Attr { code: c, .. } | Node::Bool { code: c, .. } => code(c),
        Node::RenderSnippet { args, .. } => code(args),
        Node::Snippet { body, .. } | Node::Head(body) => first_await(body),
        Node::Kept { sent, own, .. } => first_await(sent).or_else(|| all(own.iter())),
        Node::Chosen {
            own: Some(own),
            line,
            ..
        } => rust_scan::awaits(own).then_some(*line),
        Node::If {
            branches,
            otherwise,
        } => branches
            .iter()
            .find_map(|(c, b)| code(c).or_else(|| first_await(b)))
            .or_else(|| all(otherwise.iter())),
        Node::Each {
            iter,
            body,
            otherwise,
            ..
        } => code(iter)
            .or_else(|| first_await(body))
            .or_else(|| all(otherwise.iter())),
        Node::Match { scrutinee, arms } => code(scrutinee).or_else(|| {
            arms.iter()
                .find_map(|(c, b)| code(c).or_else(|| first_await(b)))
        }),
        Node::Await {
            future,
            pending,
            then,
            catch,
        } => code(future)
            .or_else(|| first_await(pending))
            .or_else(|| all(then.iter().chain(catch).map(|(_, b)| b))),
        Node::Component {
            props, children, ..
        } => props
            .iter()
            .find_map(|p| match &p.value {
                template::PropValue::Expr(c) => code(c),
                _ => None,
            })
            .or_else(|| all(children.iter())),
        Node::Client(branches) => all(branches.iter().map(|(_, b)| b)),
        _ => None,
    })
}

/// The lists of nodes right inside `n`: a block's branches, children, an
/// await's pending markup and branches.
fn inside(n: &Node) -> Vec<&Vec<Node>> {
    match n {
        Node::If {
            branches,
            otherwise,
        } => branches.iter().map(|(_, b)| b).chain(otherwise).collect(),
        Node::Each {
            body, otherwise, ..
        } => std::iter::once(body).chain(otherwise).collect(),
        Node::Match { arms, .. } => arms.iter().map(|(_, b)| b).collect(),
        Node::Kept { sent, own, .. } => std::iter::once(sent).chain(own).collect(),
        Node::Component { children, .. } => children.iter().collect(),
        Node::Await {
            pending,
            then,
            catch,
            ..
        } => std::iter::once(pending)
            .chain(then.iter().chain(catch).map(|(_, b)| b))
            .collect(),
        Node::Snippet { body, .. } | Node::Head(body) => vec![body],
        Node::Client(branches) => branches.iter().map(|(_, b)| b).collect(),
        _ => Vec::new(),
    }
}

/// The `{#await}`s in `nodes`, in blocks and children too.
fn awaits_of(nodes: &[Node]) -> Vec<&Node> {
    let mut out = Vec::new();
    for n in nodes {
        if matches!(n, Node::Await { .. }) {
            out.push(n);
        }
        for list in inside(n) {
            out.extend(awaits_of(list));
        }
    }
    out
}

/// Whether `nodes` have an `{#await}`.
fn awaits_in(nodes: &[Node]) -> bool {
    !awaits_of(nodes).is_empty()
}

/// Whether `nodes` have browser code of their template's own: `{:x}`,
/// `on:`, a browser block (a component's is its own).
fn has_browser(nodes: &[Node]) -> bool {
    nodes.iter().any(|n| {
        matches!(
            n,
            Node::Live { .. } | Node::Hole { .. } | Node::Tag { .. } | Node::Client(_)
        ) || inside(n).into_iter().any(|b| has_browser(b))
    })
}

/// A page's `{#await}` whose `{:then}` or `{:catch}` cannot render after
/// the page: the line of its future, and why.
fn bad_await(nodes: &[Node]) -> Option<(u32, &'static str)> {
    awaits_of(nodes).into_iter().find_map(|n| {
        let Node::Await {
            future,
            then,
            catch,
            ..
        } = n
        else {
            return None;
        };
        let mut branches: Vec<Node> = then.iter().chain(catch).flat_map(|(_, b)| b.clone()).collect();
        if has_browser(&branches) {
            return Some((
                future.line,
                "an `{#await}`'s `{:then}` and `{:catch}` are rendered after the page, without its browser code: \
                 no `{:x}`, `on:`, `bind:` or browser blocks of the page's in them. A component with its own script works there; \
                 or put the page's browser code around the block",
            ));
        }
        let mut cx = false;
        let _ = for_each_code(&mut branches, &mut |c| {
            cx |= names_word(&c.src, "cx");
            Ok(())
        });
        cx.then_some((
            future.line,
            "an `{#await}`'s `{:then}` and `{:catch}` are rendered after the request, so they have no `cx`: \
             read what they need before, and give it to the future (`stats(cx.param(\"id\").to_string())`)",
        ))
    })
}

/// `stmts` (a block's statements, as long as the file) with each of
/// `lets` put on its line, so rustc's errors point at the markup.
fn with_lets(stmts: Option<String>, lets: &[(String, u32)]) -> Option<String> {
    if lets.is_empty() {
        return stmts;
    }
    let mut lines: Vec<String> = stmts
        .unwrap_or_default()
        .split('\n')
        .map(String::from)
        .collect();
    for (code, line) in lets {
        for (j, part) in code.split('\n').enumerate() {
            let at = *line as usize - 1 + j;
            if lines.len() <= at {
                lines.resize(at + 1, String::new());
            }
            lines[at].push(' ');
            lines[at].push_str(part);
        }
    }
    Some(lines.join("\n"))
}

/// The generated Rust, the scoped CSS of its templates and their
/// accessibility warnings.
pub fn generate(input: &Input) -> Result<Output, String> {
    generate_web(input)
}

/// The generated Rust, and the TypeScript client of the app's endpoints
/// (empty without any). The app is read phase by phase, each finding what
/// the next needs, then written out.
pub fn generate_all(input: &Input) -> Result<(String, String), String> {
    generate_web(input).map(|o| (o.code, o.client))
}

/// What `fn before` of `src/hooks.rs` asks of requests: a bearer token, a
/// signed-in member, and whether it looks at `cx.writes()` (only changes
/// need it). A guess from its text, for the document's `security`.
fn hooks_gate(root: &Path) -> (bool, bool, bool) {
    let src = crate::read_source(&root.join("src").join("hooks.rs")).unwrap_or_default();
    let Ok(items) = rust_scan::scan(&src) else {
        return (false, false, false);
    };
    match items.fns.iter().find(|f| f.name == "before") {
        Some(f) => (f.bearer, f.session, src.contains(".writes()")),
        None => (false, false, false),
    }
}

/// The OpenAPI 3.1 document of the app's endpoints, pages and actions
/// (empty without any), for `wisp openapi`.
pub fn openapi(input: &Input) -> Result<String, String> {
    generate_web(input).map(|o| o.spec)
}

/// `key = "value"` of Cargo.toml's `[package]`.
fn package_field(toml: &str, key: &str) -> Option<String> {
    let mut in_package = false;
    for line in toml.lines().map(str::trim) {
        if line.starts_with('[') {
            in_package = line == "[package]";
        } else if in_package
            && let Some((k, v)) = line.split_once('=')
            && k.trim() == key
        {
            return Some(v.trim().trim_matches('"').to_string());
        }
    }
    None
}

/// [`generate`] for `wisp check`: what the app's browser code imports of
/// its npm packages, as esm.sh paths (for `wisp build` to download), and
/// the templates' accessibility warnings.
pub fn check(input: &Input) -> Result<(Vec<String>, Vec<String>), String> {
    let o = generate_web(input)?;
    Ok((o.web.imports(npm::ESM), o.warnings))
}

/// `wisp check --explain-imports`: what each file is auto-imported.
pub fn imports(input: &Input) -> Result<crate::auto::FileImports, String> {
    let p = Project::load(input)?;
    let web = p.browser()?;
    let mut g = Gen {
        out: String::new(),
        release: input.release,
        types: None,
        users: None,
        db: false,
        auto: Default::default(),
        imports: Vec::new(),
    };
    g.modules(&p, &web)?;
    Ok(g.imports)
}

/// One file a page of a route loads, for `wisp build --analyze`.
pub struct Weight {
    /// `js`, `css` or `wasm`.
    pub kind: &'static str,
    pub name: String,
    pub bytes: Vec<u8>,
}

/// `wisp build --analyze`: per route (its pattern), the browser files a
/// page of it loads as they are served: the runtime, the modules of its
/// templates and what they import, the CSS, and the `static/` `.wasm` files
/// its JavaScript names. A route with no page is left out.
pub fn analyze(input: &Input) -> Result<Vec<(String, Vec<Weight>)>, String> {
    let p = Project::load(input)?;
    let web = p.browser()?;
    let js = |name: &str, src: &str| Weight {
        kind: "js",
        name: name.into(),
        bytes: crate::minify_js(src).into_bytes(),
    };
    let mut css = match css_source(p.root)? {
        Some(f) => fs::read(&f).map_err(|e| format!("{}: {e}", f.display()))?,
        None => Vec::new(),
    };
    css.extend_from_slice(p.styles().as_bytes());
    let mut wasm = Vec::new();
    let static_dir = p.root.join("static");
    if static_dir.is_dir() {
        list_files(&static_dir, &mut wasm)?;
        wasm.retain(|f| f.extension().is_some_and(|e| e == "wasm"));
    }
    let served = |f: &JsFile| match &f.file {
        Some(file) => fs::read_to_string(file).unwrap_or_default(),
        None => f.source.clone(),
    };
    let mut out = Vec::new();
    for r in &p.model.routes {
        let Some(page) = &r.page else { continue };
        let tpls: Vec<usize> = (r.layouts.iter().map(|&l| p.model.layouts[l].tpl))
            .chain([page.tpl])
            .collect();
        let mut files = vec![js(WISP_JS_PATH, wisp_shared::WISP_JS)];
        let mut urls = Vec::new();
        let mut live = false;
        for c in tpls.iter().filter_map(|&t| web.clients[t].as_ref()) {
            live = true;
            urls.push(format!("{}?v={}", c.path(), c.hash));
            urls.extend(c.preload.iter().cloned());
            files.push(Weight {
                kind: "js",
                name: c.path(),
                bytes: c.source.clone().into_bytes(),
            });
        }
        if live {
            files.push(js(LIVE_JS_PATH, wisp_shared::LIVE_JS));
        }
        urls.sort();
        urls.dedup();
        for f in &web.js_files {
            let url = match f.file {
                Some(_) => f.path.clone(),
                None => format!("{}?v={}", f.path, f.hash),
            };
            if urls.contains(&url) && !files.iter().any(|w| w.name == f.path) {
                files.push(Weight {
                    kind: "js",
                    name: f.path.clone(),
                    bytes: served(f).into_bytes(),
                });
            }
        }
        if !css.is_empty() {
            files.push(Weight {
                kind: "css",
                name: APP_CSS_PATH.into(),
                bytes: css.clone(),
            });
        }
        let text: String = files
            .iter()
            .filter(|w| w.kind == "js")
            .map(|w| String::from_utf8_lossy(&w.bytes).into_owned())
            .collect();
        for f in &wasm {
            let name = f
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            if text.contains(&name) {
                let bytes = fs::read(f).map_err(|e| format!("{}: {e}", f.display()))?;
                files.push(Weight {
                    kind: "wasm",
                    name,
                    bytes,
                });
            }
        }
        out.push((r.pattern.clone(), files));
    }
    Ok(out)
}

/// For `wisp dev`: what a running dev build takes without a compile, and a
/// fingerprint of the rest.
pub struct Hot {
    /// The generated Rust but for what the running build takes as it is
    /// (templates' static text and shapes, browser files) and the
    /// `// file:line` notes: equal, a compile would make the same program.
    pub rust: u64,
    /// `src/app.html` first, then every template.
    pub templates: Vec<HotTemplate>,
    /// The browser files served: path, the URL pages name it by, source.
    pub files: Vec<(String, String, String)>,
    /// As [`generate`] gives them.
    pub warnings: Vec<String>,
}

pub struct HotTemplate {
    pub rel: String,
    pub shape: u64,
    pub chunks: Vec<String>,
    /// Of its `---` block (or `+page.rs`), and of its `{@props}`: what a
    /// change needs a compile for, to say why.
    pub block: u64,
    pub props: u64,
}

pub fn hot(input: &Input) -> Result<Hot, String> {
    let (p, web, code, _) = generate_parts(input)?;
    let mut rust = Vec::with_capacity(code.len());
    for line in code.lines() {
        let l = line.trim_start();
        if l.starts_with("static __WISP_S:")
            || l.starts_with("const TEMPLATES:")
            || l.starts_with("pub static S: [&str; 3]")
            || l.starts_with("static BAKED_")
            || l.contains("::wisp::ClientModule = ::wisp::ClientModule {")
        {
            // But the messages a module's script shows, which the binary
            // sends with the page.
            if let Some(at) = l.rfind(", texts: &[") {
                rust.extend_from_slice(&l.as_bytes()[at..]);
                rust.push(b'\n');
            }
            continue;
        }
        rust.extend_from_slice(without_note(line).as_bytes());
        rust.push(b'\n');
    }
    let mut templates = vec![HotTemplate {
        rel: "src/app.html".into(),
        shape: shell::shape(&p.shell),
        chunks: p.shell.to_vec(),
        block: 0,
        props: 0,
    }];
    for t in p.templates.iter() {
        let inline = (p.user_mods.iter())
            .find(|m| p.rel(&m.file) == t.rel)
            .and_then(|m| m.inline.as_deref());
        let block = format!(
            "{}\0{}",
            inline.unwrap_or(""),
            t.stmts.as_ref().map_or("", |s| s.0.as_str())
        );
        let mut props = Vec::new();
        prop_decls(&t.t, &mut props);
        templates.push(HotTemplate {
            rel: t.rel.clone(),
            shape: t.t.shape,
            chunks: t.t.chunks.clone(),
            block: fnv1a(block.as_bytes()),
            props: fnv1a(&props),
        });
    }
    let mut files: Vec<(String, String, String)> = (web.clients.iter().flatten())
        .map(|c| {
            (
                c.path(),
                format!("{}?v={}", c.path(), c.hash),
                c.source.clone(),
            )
        })
        .collect();
    files.extend((web.js_files.iter()).filter(|f| f.file.is_none()).map(|f| {
        (
            f.path.clone(),
            format!("{}?v={}", f.path, f.hash),
            f.source.clone(),
        )
    }));
    Ok(Hot {
        rust: fnv1a(&rust),
        templates,
        files,
        warnings: p.warnings(),
    })
}

/// A generated line without the ` // src/x.wisp:12` note at its end, which
/// moves with every line added above it.
fn without_note(line: &str) -> &str {
    match line.rfind(" // src/") {
        Some(i)
            if line[i + 4..].split_once(':').is_some_and(|(f, n)| {
                !f.contains(' ') && !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit())
            }) =>
        {
            &line[..i]
        }
        _ => line,
    }
}

/// A template's `{@props}`, as its shape has them.
fn prop_decls(t: &Template, out: &mut Vec<u8>) {
    for d in t.props.iter().flat_map(|(ds, _)| ds) {
        let default = d.default.as_deref().unwrap_or("");
        out.extend_from_slice(format!("{}\0{}\0{default}\0", d.name, d.ty).as_bytes());
    }
}

/// For `wisp check --types`: what `.wisp/types` holds for `tsc`, by path
/// there. Each `<script lang="ts">` is a module of its own,
/// `src/routes/+page.wisp.ts`, on its lines of the file, with the server
/// values or props it reads declared at its end (a `#[derive(Json)]`
/// type's fields as theirs, a block's `let`s as `probed` has them, or as
/// written, else `any`); `wisp.d.ts` has the runes and helpers.
///
/// `probed` is what an app built with `types` printed (see `wisp::ts`):
/// `{"file":{"values":[[name, type]..],"decls":[[name, decl]..]}..}`. The
/// `bool` says whether a script reads a block's values, which only it types.
pub fn types(input: &Input, probed: &str) -> Result<(Vec<(String, String)>, bool), String> {
    let p = Project::load(input)?;
    let probed = match probed {
        "" => wisp_shared::json::Json::Obj(Vec::new()),
        text => wisp_shared::json::parse(text).map_err(|e| format!("The app's types: {e}"))?,
    };
    let pairs = |v: Option<&wisp_shared::json::Json>| -> Vec<(String, String)> {
        v.into_iter()
            .flat_map(|v| v.items())
            .filter_map(|p| {
                let mut p = p.items().filter_map(|s| s.as_str());
                Some((p.next()?.to_string(), p.next()?.to_string()))
            })
            .collect()
    };
    let mut wanted = false;
    let mut out = vec![("wisp.d.ts".to_string(), WISP_D_TS.to_string())];
    for t in &p.templates {
        let Some((s, written)) = (t.t.script.as_ref()).and_then(|s| Some((s, s.ts.as_deref()?)))
        else {
            continue;
        };
        let mut f = "\n".repeat(s.line as usize - 1) + &" ".repeat(s.col as usize - 1);
        f.push_str(written);
        f.push_str("\nexport {};\n");
        // What the script declares is its own.
        let mut own: Vec<String> = js::declarations(&s.src)
            .into_iter()
            .map(|(n, _)| n)
            .collect();
        own.extend(js::import_names(&s.src, &js::imports(&s.src)));
        let types = p.types_of(t);
        let mut decls = Vec::new();
        let mut values: Vec<(String, String)> = Vec::new();
        let mut ts = |ty: &str| openapi::ts(ty, &types, &mut decls);
        match t.kind {
            Kind::Component => {
                for d in t.t.props.iter().flat_map(|(ds, _)| ds) {
                    values.push((d.name.clone(), ts(&d.ty)));
                }
            }
            Kind::Page | Kind::Layout if matches!(t.user, Some((_, true))) => {
                let mut fields: Vec<(String, String)> =
                    t.data.iter().map(|(n, ty)| (n.clone(), ts(ty))).collect();
                if let Some((stmts, binds)) = &t.stmts {
                    wanted = true;
                    let here = probed.get(&t.rel);
                    let known = pairs(here.and_then(|h| h.get("values")));
                    let lets = rust_scan::let_names(stmts).into_iter();
                    for n in lets.chain(rust_scan::let_names(&binds.join("\n"))) {
                        if !n.starts_with("__") && !fields.iter().any(|f| f.0 == n) {
                            let ty = match known.iter().find(|(k, _)| *k == n) {
                                Some((_, ty)) => ty.clone(),
                                None => {
                                    annotated(stmts, &n).map_or_else(|| "any".into(), |r| ts(&r))
                                }
                            };
                            fields.push((n, ty));
                        }
                    }
                    for (n, d) in pairs(here.and_then(|h| h.get("decls"))) {
                        if !decls.iter().any(|(k, _)| *k == n) {
                            decls.push((n, d + "\n"));
                        }
                    }
                }
                let shape: Vec<String> = fields.iter().map(|(n, t)| format!("{n}: {t}")).collect();
                // A `+page.js` makes `data` in the browser.
                let data = match t.load_js {
                    Some(_) => "any".to_string(),
                    None => format!("{{ {} }}", shape.join("; ")),
                };
                values.push(("data".into(), data));
                values.extend(fields);
            }
            _ => {}
        }
        for (n, ty) in values {
            if !own.contains(&n) && !js::is_reserved(&n) && !js::is_global(&n) {
                let _ = writeln!(f, "declare const {n}: {ty};");
            }
        }
        // `$cart` is store `cart`'s value.
        let mut stores: Vec<&str> = Vec::new();
        for tok in js::tokens(&s.src) {
            let w = tok.text(&s.src);
            if let Some(b) = w.strip_prefix('$')
                && own.iter().any(|n| n == b)
                && !stores.contains(&w)
            {
                stores.push(w);
                let _ = writeln!(
                    f,
                    "declare let {w}: typeof {b} extends {{ value: infer V }} ? V : never;"
                );
            }
        }
        for (_, d) in decls {
            f.push_str(&d);
        }
        out.push((format!("{}.ts", t.rel), f));
    }
    if !p.remotes.is_empty() {
        out.push(("remote.d.ts".into(), remote_ts(&p.remotes)?));
    }
    Ok((out, wanted))
}

/// The `#[remote]` functions as TypeScript: global, as scripts call them,
/// and as `wisp:remote`'s exports. An `Option` argument at the end may be
/// left out; what answers `None` (a 404) rejects, so it is not in the
/// promise.
fn remote_ts(remotes: &[RemoteFn]) -> Result<String, String> {
    let mut decls = Vec::new();
    let mut sigs = Vec::new();
    for r in remotes {
        let inputs = r.f.inputs().map_err(|e| format!("{}:{e}", r.rel))?;
        let mut params = Vec::new();
        let mut trailing = true;
        for (n, ty) in inputs.iter().rev() {
            trailing &= ty::option_inner(ty).is_some();
            let q = if trailing { "?" } else { "" };
            params.push(format!("{n}{q}: {}", openapi::ts(ty, &r.types, &mut decls)));
        }
        params.reverse();
        let value = r.f.value_type();
        let value = ty::option_inner(value).unwrap_or(value);
        let ret = match rust_scan::returns_kind(value) {
            Returns::Nothing => "void".to_string(),
            Returns::Other if ty::last_segment(value) != "Image" => {
                openapi::ts(value, &r.types, &mut decls)
            }
            _ => "unknown".to_string(),
        };
        sigs.push(format!(
            "function {}({}): Promise<{ret}>;",
            r.f.name,
            params.join(", ")
        ));
    }
    let mut f =
        String::from("// Written by `wisp check --types`: the app's #[remote] functions.\n");
    for s in &sigs {
        let _ = writeln!(f, "declare {s}");
    }
    f.push_str("declare module 'wisp:remote' {\n");
    for s in &sigs {
        let _ = writeln!(f, "  export {s}");
    }
    f.push_str("}\n");
    for (_, d) in decls {
        f.push_str(&d);
    }
    Ok(f)
}

/// The Rust type of `let name: T = …` in `stmts`, if it is written.
fn annotated(stmts: &str, name: &str) -> Option<String> {
    let at = [format!("let {name}:"), format!("let mut {name}:")]
        .iter()
        .find_map(|p| stmts.find(p.as_str()).map(|i| i + p.len()))?;
    let rest = &stmts[at..];
    let mut depth = 0i32;
    let end = rest.char_indices().find_map(|(i, c)| {
        match c {
            '<' | '(' | '[' => depth += 1,
            '>' | ')' | ']' => depth -= 1,
            '=' | ';' if depth == 0 => return Some(i),
            _ => {}
        }
        None
    })?;
    Some(rest[..end].trim().to_string()).filter(|t| !t.is_empty())
}

/// The runes, the helpers every script has, and the `wisp` module, as
/// TypeScript sees them (`wisp check --types`).
const WISP_D_TS: &str = "// Written by `wisp check --types`: Wisp's browser helpers, for tsc.
declare function $state<T>(value: T): T;
declare function $state<T>(): T | undefined;
declare namespace $state {
  function raw<T>(value: T): T;
  function snapshot<T>(value: T): T;
}
declare function $derived<T>(value: T): T;
declare namespace $derived {
  function by<T>(f: () => T): T;
}
declare function $effect(f: () => void | (() => void)): void;
declare namespace $effect {
  function pre(f: () => void | (() => void)): void;
}
declare function $props(): any;
declare function $bindable<T>(value?: T): T;
declare function $inspect(...values: unknown[]): void;
declare function onMount(f: () => void | (() => void) | Promise<void>): void;
declare function onDestroy(f: () => void): void;
declare function effect(f: () => void | (() => void), deps?: () => unknown[]): void;
declare function watch<T>(read: () => T, f: (value: T) => void): void;
declare function listen(url: string, f: (data: any) => void): void;
declare function emit(name: string, value?: unknown): void;
declare function setContext(key: unknown, value: unknown): void;
declare function getContext<T = any>(key: unknown): T;
declare function tick(): Promise<void>;
declare function untrack<T>(f: () => T): T;
declare function flushSync(): void;
declare function onError(f: (error: unknown) => void): () => void;
declare function tweened<T extends number | number[] | Record<string, number>>(value: T, o?: { duration?: number; delay?: number; easing?: (t: number) => number }): { value: T; set(v: T, o?: { duration?: number; delay?: number; easing?: (t: number) => number }): Promise<void>; update(f: (v: T) => T): Promise<void>; subscribe(f: (v: T) => void): () => void };
declare function spring<T extends number | number[] | Record<string, number>>(value: T, o?: { stiffness?: number; damping?: number; precision?: number }): { value: T; set(v: T, o?: { hard?: boolean }): Promise<void>; update(f: (v: T) => T): Promise<void>; subscribe(f: (v: T) => void): () => void };
declare function crossfade(o?: { duration?: number; easing?: (t: number) => number }): [(el: Element, o: { key: unknown }) => any, (el: Element, o: { key: unknown }) => any];
declare function goto(url: string | URL, opts?: { replace?: boolean; noscroll?: boolean; keepfocus?: boolean }): Promise<void>;
declare function invalidate(dep?: string): Promise<void>;
declare function matches(text: unknown, q: unknown): boolean;
declare function enhance(form: HTMLFormElement, submit?: (e: any) => any): void;
declare function context<T = any>(): [() => T, (value: T) => void];
declare function pushState(url: string | URL, state?: any): void;
declare function replaceState(url: string | URL, state?: any): void;
declare const page: { value: { url: URL; status: number; form: any; state: any } };
declare const navigating: { value: { from: URL; to: URL } | null };
declare const env: { readonly [name: `PUBLIC_${string}`]: string };
declare function t(key: string, values?: any): string;
declare module 'wisp' {
  export interface Store<T> {
    value: T;
    set(value: T): void;
    update(f: (value: T) => T): void;
    subscribe(f: (value: T) => void): () => void;
  }
  export function store<T>(value: T): Store<T>;
  export function persisted<T>(key: string, value: T): Store<T>;
  export function derived<T>(f: () => T): Store<T>;
  export function context<T = any>(): [() => T, (value: T) => void];
  export function untrack<T>(f: () => T): T;
  export function flushSync(): void;
  export function onError(f: (error: unknown) => void): () => void;
  export function tick(): Promise<void>;
  export function goto(url: string | URL, opts?: { replace?: boolean; noscroll?: boolean; keepfocus?: boolean }): Promise<void>;
  export function invalidate(dep?: string): Promise<void>;
  export function invalidateAll(): Promise<void>;
  export function beforeNavigate(f: (nav: { from: URL; to: URL; pop: boolean; cancel(): void }) => void): () => void;
  export function afterNavigate(f: (nav: { from: URL; to: URL }) => void): () => void;
  export function onNavigate(f: (nav: { from: URL; to: URL }) => void | (() => void) | Promise<void | (() => void)>): () => void;
  export function preloadData(url: string | URL): Promise<void>;
  export function preloadCode(url: string | URL): Promise<void>;
  export const updated: Store<boolean>;
  export function matches(text: unknown, q: unknown): boolean;
  export function pushState(url: string | URL, state?: any): void;
  export function replaceState(url: string | URL, state?: any): void;
  export const page: Store<{ url: URL; status: number; form: any; state: any }>;
  export const navigating: Store<{ from: URL; to: URL } | null>;
}
// An npm package or a URL: what it exports is not known here.
declare module '*';
";

pub struct Output {
    pub code: String,
    client: String,
    spec: String,
    web: Web,
    pub styles: String,
    /// `file:line: what (a11y-name)`, in file order.
    pub warnings: Vec<String>,
}

fn generate_web(input: &Input) -> Result<Output, String> {
    let (p, web, code, docs) = generate_parts(input)?;
    Ok(Output {
        code,
        client: docs.0,
        spec: docs.1,
        styles: p.styles(),
        warnings: p.warnings(),
        web,
    })
}

/// The app read, its browser half, the generated Rust and the TypeScript
/// client.
fn generate_parts<'a>(
    input: &Input<'a>,
) -> Result<(Project<'a>, Web, String, (String, String)), String> {
    let mut p = Project::load(input)?;
    let web = p.browser()?;
    if input.release {
        p.stamp();
    }
    let mut g = Gen {
        out: String::new(),
        release: input.release,
        types: (!input.release).then(Vec::new),
        users: None,
        db: false,
        auto: Default::default(),
        imports: Vec::new(),
    };
    g.modules(&p, &web)?;
    g.servers(&p);
    let assets = g.assets(&p)?;
    let docs = g.app(&p, &web, &assets)?;
    Ok((p, web, g.out, docs))
}

/// The app's own modules: each `src/NAME.rs` that `main.rs` and `lib.rs` do
/// not declare themselves (with `mod NAME;`), but for `main.rs`, `lib.rs`
/// and `hooks.rs`. Wisp compiles each as `crate::NAME` with the prelude in
/// scope, and route files and templates reach it as `NAME`.
fn app_mods(root: &Path) -> Result<(Vec<UserMod>, Vec<RemoteFn>), String> {
    let src = root.join("src");
    let mut declared = String::new();
    for f in ["main.rs", "lib.rs"] {
        declared.push_str(&crate::read_source(&src.join(f)).unwrap_or_default());
        declared.push('\n');
    }
    let (mut out, mut remotes) = (Vec::new(), Vec::new());
    let Ok(dir) = fs::read_dir(&src) else {
        return Ok((out, remotes));
    };
    let mut files: Vec<PathBuf> = dir.flatten().map(|e| e.path()).collect();
    files.sort();
    for file in files {
        let Some(name) = file
            .file_name()
            .and_then(|n| n.to_str())
            .and_then(|n| n.strip_suffix(".rs"))
        else {
            continue;
        };
        let ident = ty::is_ident(name) && !name.starts_with(|c: char| c.is_ascii_uppercase());
        if !file.is_file()
            || !ident
            || matches!(name, "main" | "lib" | "hooks")
            || declares_mod(&declared, name)
        {
            continue;
        }
        let items =
            rust_scan::scan(&crate::read_source(&file).map_err(|e| format!("src/{name}.rs: {e}"))?)
                .map_err(|e| format!("src/{name}.rs:{e}"))?;
        items
            .check_inner()
            .map_err(|e| format!("src/{name}.rs:{e}"))?;
        let mut shims = Vec::new();
        for f in items.fns.iter().filter(|f| f.remote.is_some()) {
            shims.push(remote_shim(f).map_err(|e| format!("src/{name}.rs:{e}"))?);
            remotes.push(RemoteFn {
                f: f.clone(),
                module: format!("__mods::{name}"),
                rel: format!("src/{name}.rs"),
                types: Vec::new(),
            });
        }
        out.push(UserMod::new(name.into(), file, None, shims, &items));
    }
    Ok((out, remotes))
}

/// The app's own modules Wisp compiles (`src/notes.rs`), by name, as far
/// as they can be read.
pub(crate) fn mod_files(root: &Path) -> Vec<(String, PathBuf)> {
    let mods = app_mods(root).map(|m| m.0).unwrap_or_default();
    mods.into_iter().map(|m| (m.name, m.file)).collect()
}

/// Whether `src` has `mod NAME;` (`pub mod`, with attributes, anywhere).
/// The state a handler that does nothing else makes of its name: `open =
/// !open` is `open`, starting ` = false`; `n++` or `n--`, ` = 0`.
fn state_of(handler: &str) -> Option<(&str, &str)> {
    let h = handler.trim().trim_end_matches(';').trim_end();
    let counted = (h.strip_suffix("++").or_else(|| h.strip_suffix("--")))
        .map(str::trim_end)
        .or_else(|| (h.strip_prefix("++").or_else(|| h.strip_prefix("--"))).map(str::trim_start));
    if let Some(n) = counted {
        return ty::is_ident(n).then_some((n, " = 0"));
    }
    let (name, rest) = h.split_once('=')?;
    let name = name.trim_end();
    let flipped = rest.trim_start().strip_prefix('!')?.trim();
    (ty::is_ident(name) && flipped == name).then_some((name, " = false"))
}

fn declares_mod(src: &str, name: &str) -> bool {
    src.match_indices("mod ").any(|(i, _)| {
        let before = src[..i].chars().next_back();
        let rest = src[i + 4..].trim_start();
        before.is_none_or(|c| !(c.is_alphanumeric() || c == '_'))
            && rest
                .strip_prefix(name)
                .is_some_and(|r| r.trim_start().starts_with(';'))
    })
}

/// Whether `name` appears in `src` as a whole word.
fn names_word(src: &str, name: &str) -> bool {
    let word = |c: Option<char>| c.is_some_and(|c| c.is_alphanumeric() || c == '_');
    src.match_indices(name).any(|(i, _)| {
        !word(src[..i].chars().next_back()) && !word(src[i + name.len()..].chars().next())
    })
}

/// The names of a block's values its browser code reads, as `data.name`.
fn data_names(client: Option<&Client>) -> Vec<&str> {
    client
        .iter()
        .flat_map(|c| &c.blob)
        .filter_map(|p| match p {
            Piece::Value { expr, .. } => expr.strip_prefix("data."),
            Piece::Text(_) => None,
        })
        .map(|rest| rest.split('.').next().unwrap_or(rest))
        .fold(Vec::new(), |mut v, n| {
            if !v.contains(&n) {
                v.push(n);
            }
            v
        })
}

fn opt(x: Option<usize>) -> String {
    x.map_or("None".into(), |i| format!("Some({i})"))
}

/// The components of the app at `root`, as `check` reads them.
pub(crate) fn components(root: &Path) -> Result<Vec<Comp>, String> {
    let mut p = Project::new(&Input {
        root,
        release: false,
        maps: false,
        prerendered: None,
    })?;
    p.components()?;
    Ok(p.comps)
}

/// The Markdown pages in the order `wisp::pages` gives them: by folder,
/// then newest `date` first, then by path.
fn md_order(pages: &[(String, Vec<(String, String)>)]) -> Vec<&(String, Vec<(String, String)>)> {
    let dir = |p: &str| p.rsplit_once('/').map_or("", |(d, _)| d).to_string();
    let date = |f: &[(String, String)]| {
        (f.iter().find(|(k, _)| k == "date")).map_or(String::new(), |(_, v)| v.clone())
    };
    let mut out: Vec<_> = pages.iter().collect();
    out.sort_by(|a, b| {
        (dir(&a.0).cmp(&dir(&b.0)))
            .then_with(|| date(&b.1).cmp(&date(&a.1)))
            .then_with(|| a.0.cmp(&b.0))
    });
    out
}

/// The markup has `<meta name="robots" content="noindex">` (any case,
/// any order): the sitemap leaves the page out.
fn noindex(markup: &str) -> bool {
    let lower = markup.to_ascii_lowercase();
    lower.split("<meta").skip(1).any(|m| {
        let tag = &m[..m.find('>').unwrap_or(m.len())];
        tag.contains("robots") && tag.contains("noindex")
    })
}

fn check_no_children(t: &Template, rel: &str) -> Result<(), String> {
    if t.uses_children {
        Err(format!(
            "{rel}: only layouts and components can use {{@render children()}}"
        ))
    } else {
        Ok(())
    }
}

/// The Rust type of a prop only `$props()` names: any browser value.
const DYN: &str = "&dyn ::wisp::Json";
/// The prop that holds the rest, for `...rest`: `[name, value]` pairs.
const REST: &str = "__rest";

/// The props of a component with `$props()` and no `{@props}`: every one
/// optional, a browser value whose default the server knows when it is a
/// literal (else `null`, which the browser's default replaces).
fn inferred_props(r: &js::PropsRune) -> Vec<PropDecl> {
    let mut out: Vec<PropDecl> = r
        .props
        .iter()
        .map(|p| {
            let json = p
                .default
                .as_deref()
                .and_then(|d| literal_json(d, &js::tokens(d)))
                .unwrap_or_else(|| "null".into());
            PropDecl {
                name: p.name.clone(),
                ty: DYN.into(),
                default: Some(format!("&::wisp::rt::Js({})", lit(&json))),
            }
        })
        .collect();
    if r.rest.is_some() {
        out.push(PropDecl {
            name: REST.into(),
            ty: format!("&[(&str, {DYN})]"),
            default: Some("&[]".into()),
        });
    }
    out
}

/// Every component used in `nodes` exists and is given what it takes.
pub(crate) fn check_components(
    nodes: &[Node],
    t: &Template,
    comps: &[Comp],
    rel: &str,
    in_head: bool,
) -> Result<(), String> {
    for n in nodes {
        match n {
            Node::If {
                branches,
                otherwise,
            } => {
                for body in branches.iter().map(|b| &b.1).chain(otherwise) {
                    check_components(body, t, comps, rel, in_head)?;
                }
            }
            Node::Each {
                body, otherwise, ..
            } => {
                for body in std::iter::once(body).chain(otherwise) {
                    check_components(body, t, comps, rel, in_head)?;
                }
            }
            Node::Kept { sent, own, .. } => {
                for body in std::iter::once(sent).chain(own) {
                    check_components(body, t, comps, rel, in_head)?;
                }
            }
            Node::Match { arms, .. } => {
                for (_, body) in arms {
                    check_components(body, t, comps, rel, in_head)?;
                }
            }
            Node::Await {
                pending,
                then,
                catch,
                ..
            } => {
                let branches = then.iter().chain(catch).map(|(_, b)| b);
                for body in std::iter::once(pending).chain(branches) {
                    check_components(body, t, comps, rel, in_head)?;
                }
            }
            Node::Head(body) => check_components(body, t, comps, rel, true)?,
            Node::Snippet { body, .. } => check_components(body, t, comps, rel, in_head)?,
            Node::Client(branches) => {
                for (_, body) in branches {
                    check_components(body, t, comps, rel, in_head)?;
                }
            }
            Node::Component {
                name,
                props,
                children,
                line,
            } => {
                let at = |msg: String| format!("{rel}:{line}: {msg}");
                if in_head {
                    return Err(at(format!(
                        "<{name}> is a component, which cannot go in <wisp:head>; write the tags there directly"
                    )));
                }
                let Some(c) = comps.iter().find(|c| c.name == *name) else {
                    let known: Vec<&str> = comps.iter().map(|c| c.name.as_str()).collect();
                    let known = if known.is_empty() {
                        "there are none yet".to_string()
                    } else {
                        format!("there are {}", known.join(", "))
                    };
                    return Err(at(format!(
                        "no component `{name}`: components are the .wisp files in src/components, and {known}"
                    )));
                };
                let takes = || {
                    let names: Vec<&str> = c.props.iter().map(|d| d.name.as_str()).collect();
                    if names.is_empty() {
                        "none".to_string()
                    } else {
                        names.join(", ")
                    }
                };
                for p in props {
                    if p.name.starts_with("client:") {
                        if !c.live {
                            return Err(at(format!(
                                "<{name}> has no browser code, so `{}` has nothing to load; it is plain HTML already",
                                p.name
                            )));
                        }
                        continue;
                    }
                    let Some(d) = c.props.iter().find(|d| d.name == p.name && d.name != REST)
                    else {
                        if c.rest {
                            continue;
                        }
                        return Err(at(format!(
                            "<{name}> has no prop `{}`; it takes {}",
                            p.name,
                            takes()
                        )));
                    };
                    if p.value == PropValue::Flag && d.ty != "bool" && d.ty != DYN {
                        return Err(at(format!(
                            "`{}` alone means true, but <{name}>'s `{}` is a `{}`: write {}={{…}}",
                            p.name, d.name, d.ty, p.name
                        )));
                    }
                }
                if let Some(d) = c
                    .props
                    .iter()
                    .find(|d| d.default.is_none() && !props.iter().any(|p| p.name == d.name))
                {
                    return Err(at(format!(
                        "<{name}> needs `{}` (a `{}`): <{name} {}={{…}}>",
                        d.name, d.ty, d.name
                    )));
                }
                if let Some(children) = children {
                    let blank = children.iter().all(|n| match n {
                        Node::Text(i) => t.chunks[*i].trim().is_empty(),
                        Node::Snippet { .. } => true,
                        _ => false,
                    });
                    if !c.children && !blank {
                        return Err(at(format!(
                            "<{name}> does not show children: its template has no {{@render children()}}"
                        )));
                    }
                    check_components(children, t, comps, rel, in_head)?;
                }
            }
            _ => {}
        }
    }
    Ok(())
}

/// The files served from `static/`, by URL: the app's, then its layers'
/// (`extends`), a file the app has winning.
fn static_files(root: &Path) -> Result<Vec<(String, PathBuf)>, String> {
    let mut found: Vec<(String, PathBuf)> = Vec::new();
    let dirs = std::iter::once(root.to_path_buf()).chain(crate::plugins::layers(root));
    for dir in dirs.map(|d| d.join("static")).filter(|d| d.is_dir()) {
        let mut all = Vec::new();
        list_files(&dir, &mut all)?;
        all.sort();
        for f in all {
            let path = f.strip_prefix(&dir).unwrap_or(&f);
            let url = format!(
                "/{}",
                encode_path(&path.to_string_lossy().replace('\\', "/"))
            );
            if !found.iter().any(|(u, _)| *u == url) {
                found.push((url, f));
            }
        }
    }
    Ok(found)
}

/// The CSS to serve at `/_app/app.css`: the CSS tool's output if present,
/// otherwise `src/app.css` as written.
fn css_source(root: &Path) -> Result<Option<PathBuf>, String> {
    let built = root.join(".wisp").join("app.css");
    let src = root.join("src").join("app.css");
    if built.exists() {
        return Ok(Some(built));
    }
    if src.exists() {
        let text = crate::read_source(&src).map_err(|e| format!("{}: {e}", src.display()))?;
        // A warning for Cargo, so only in a build script: `wisp mcp` and
        // `wisp lsp` speak a protocol on stdout.
        if crate::uses_tailwind(&text) && std::env::var_os("OUT_DIR").is_some() {
            println!(
                "cargo::warning=src/app.css uses Tailwind but .wisp/app.css is missing; run `wisp dev` or `wisp build`"
            );
        }
        return Ok(Some(src));
    }
    Ok(None)
}

/// Each file's hash (`{:016x}`), read and hashed on every core: a site's
/// images and fonts are most of what a release build reads.
fn hash_files(files: &[PathBuf]) -> Vec<Result<String, String>> {
    let cores = std::thread::available_parallelism().map_or(1, |n| n.get());
    let per = files.len().div_ceil(cores).max(1);
    let hash = |f: &PathBuf| {
        fs::read(f)
            .map(|b| image::hash(&b))
            .map_err(|e| format!("{}: {e}", f.display()))
    };
    std::thread::scope(|s| {
        let parts: Vec<_> = files
            .chunks(per)
            .map(|part| s.spawn(move || part.iter().map(hash).collect::<Vec<_>>()))
            .collect();
        parts
            .into_iter()
            .flat_map(|p| p.join().expect("hashing a file does not panic"))
            .collect()
    })
}

/// The templates' images a release build serves from `/_app/img/`: a
/// `src/lib` one's original, and the WebP widths `wisp build` wrote, when
/// all are there (as `image::rewrite` names them).
fn images(root: &Path, files: &mut Vec<(String, PathBuf, String)>) {
    let dir = root.join(image::DIR);
    for found in image::sources(root) {
        let Ok(bytes) = fs::read(&found.file) else {
            continue;
        };
        let hash = image::hash(&bytes);
        let mut add = |url: String, file: PathBuf, etag: String| {
            if !files.iter().any(|f| f.0 == url) {
                files.push((url, file, etag));
            }
        };
        if found.lib {
            add(
                image::lib_url(&hash, &found.file),
                found.file.clone(),
                hash.clone(),
            );
        }
        let Some(size) = image::size(&bytes).filter(|s| found.webp && !s.turned) else {
            continue;
        };
        let widths = image::widths(size.width);
        let names: Vec<String> = widths.iter().map(|&w| image::webp_name(&hash, w)).collect();
        if names.iter().all(|n| dir.join(n).is_file()) {
            for n in names {
                let etag = n.trim_end_matches(".webp").to_string();
                add(format!("{IMAGES}{n}"), dir.join(n), etag);
            }
        }
    }
    // The icon's widths, for the manifest.
    for (_, n) in image::icon(root).map(|i| i.1).unwrap_or_default() {
        let url = format!("{IMAGES}{n}");
        if dir.join(&n).is_file() && !files.iter().any(|f| f.0 == url) {
            let etag = n.trim_end_matches(".webp").to_string();
            files.push((url, dir.join(n), etag));
        }
    }
}

pub(crate) fn list_files(dir: &Path, out: &mut Vec<PathBuf>) -> Result<(), String> {
    for e in fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))? {
        let p = e.map_err(|e| e.to_string())?.path();
        if p.is_dir() {
            list_files(&p, out)?;
        } else {
            out.push(p);
        }
    }
    Ok(())
}

/// Percent-encodes what browsers encode in a URL path (the WHATWG path
/// percent-encode set plus non-ASCII), so literals match raw request paths.
pub fn encode_path(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for &b in s.as_bytes() {
        if b <= b' '
            || b >= 0x7f
            || matches!(b, b'"' | b'#' | b'<' | b'>' | b'?' | b'`' | b'{' | b'}')
        {
            let _ = write!(out, "%{b:02X}");
        } else {
            out.push(b as char);
        }
    }
    out
}

/// A Rust string literal. Debug formatting of `str` is valid Rust syntax.
fn lit(s: &str) -> String {
    format!("{s:?}")
}

/// A field path like `data.posts` or `post.tags.0`. Iterating or matching on
/// one borrows it, so `{#each data.posts as p}` does not try to move out of
/// `data`. Bare identifiers and other expressions are used as written.
fn is_field_path(s: &str) -> bool {
    let b = s.as_bytes();
    s.contains('.')
        && (b[0].is_ascii_alphabetic() || b[0] == b'_')
        && !s.ends_with('.')
        && !s.contains("..")
        && b.iter().all(|&c| ty::is_word(c) || c == b'.')
}

/// A field path, or one of `locals`: the names a `---` block's statements
/// bind, which the markup (a closure that may run more than once) can only
/// borrow.
fn is_place(expr: &str, locals: &[String]) -> bool {
    is_field_path(expr) || locals.iter().any(|l| l == expr)
}

fn borrow_place(expr: &str, locals: &[String]) -> String {
    if is_place(expr, locals) {
        format!("&({expr})")
    } else {
        expr.to_string()
    }
}

/// What an action refused was sent as, for its form's field `name`: an
/// `Option<Cow<str>>`, which is never there without the request, `cx`.
fn kept(name: &str, has_cx: bool) -> String {
    match has_cx {
        true => format!("::wisp::rt::kept(cx, __refused, {name:?})"),
        false => "None::<::std::borrow::Cow<'static, str>>".into(),
    }
}

/// `let PAT = EXPR` with EXPR borrowed when it is a place.
fn if_condition(cond: &str, locals: &[String]) -> String {
    let Some(rest) = cond
        .strip_prefix("let")
        .filter(|r| r.starts_with(char::is_whitespace))
    else {
        // A bare place tests its truthiness: `{#if user.avatar}`.
        return match is_place(cond.trim(), locals) {
            true => format!("::wisp::rt::truthy(&({}))", cond.trim()),
            false => cond.to_string(),
        };
    };
    let b = rest.as_bytes();
    let mut eq = None;
    template::for_each_top(rest, |i| {
        let plain = b[i] == b'='
            && b.get(i + 1).is_none_or(|&c| c != b'=' && c != b'>')
            && (i == 0 || !matches!(b[i - 1], b'=' | b'!' | b'<' | b'>' | b'.'));
        if plain && eq.is_none() {
            eq = Some(i);
        }
    });
    match eq {
        Some(i) => format!(
            "let {} = {}",
            rest[..i].trim(),
            borrow_place(rest[i + 1..].trim(), locals)
        ),
        None => cond.to_string(),
    }
}
