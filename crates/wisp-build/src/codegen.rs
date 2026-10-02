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
use crate::openapi::Op;
use crate::protocol::{
    APP_CSS_PATH, COPY_END, COPY_START, EXTRA_JS_PATH, GROUP_ATTR, ISLAND_MEDIA, LIVE_JS_PATH,
    LOOP_ATTR, MODULES, ON_FLAGS, ON_PLACED, ON_ROOT, SLOT_ATTR, WISP_JS_PATH,
};
use crate::routes::Seg;
use crate::rust_scan::{self, FnItem, Returns};
use crate::template::{self, Code, Dir, Directive, Node, PropDecl, PropValue, Template};
use crate::{fnv1a, fold, js, rules, shell, ty};
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

pub struct Input<'a> {
    pub root: &'a Path,
    pub release: bool,
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
struct Comp {
    name: String,
    module: String,
    props: Vec<PropDecl>,
    /// Shows its children: `{@render children()}`.
    children: bool,
    /// The props a parent may `bind:`: those its script's `$props()`
    /// marks `$bindable`, or any, without one.
    bindable: Option<Vec<String>>,
    /// Takes any prop (its `$props()` has `...rest`): the ones it does not
    /// name go in its `__rest`.
    rest: bool,
    /// Has browser code, so `client:visible` and the like have a module
    /// to load late.
    live: bool,
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
        }
    }

    /// Its `__call` module: the shims, and `__ready`, which loads its
    /// tables.
    fn calls(&self) -> Vec<String> {
        let mut out = self.shims.clone();
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
        self.shims
            .iter()
            .any(|s| s.starts_with(&format!("pub async fn {name}(")))
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
    if let ([(_, v, get, owned)], true) = (read.as_slice(), f.checks.is_empty()) {
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

pub fn generate(input: &Input) -> Result<String, String> {
    generate_all(input).map(|(code, _)| code)
}

/// The generated Rust, and the TypeScript client of the app's endpoints
/// (empty without any). The app is read phase by phase, each finding what
/// the next needs, then written out.
pub fn generate_all(input: &Input) -> Result<(String, String), String> {
    let p = Project::load(input)?;
    let web = p.browser()?;
    let mut g = Gen {
        out: String::new(),
        release: input.release,
    };
    g.modules(&p, &web)?;
    g.servers(&p);
    let assets = g.assets(&p)?;
    let client = g.app(&p, &web, &assets)?;
    Ok((g.out, client))
}

/// The app, as far as it has been read.
struct Project<'a> {
    root: &'a Path,
    release: bool,
    tree: crate::routes::Tree,
    /// `src/app.html` (or the default) in its three pieces.
    shell: [String; 3],
    comps: Vec<Comp>,
    /// Every template but the shell, which is template 0: `templates[k]`
    /// is template `k + 1`.
    templates: Vec<Tpl>,
    /// The modules of the app's route files, and the shims in them.
    user_mods: Vec<UserMod>,
    /// The routes, layouts and error pages, as codegen prints them.
    model: Model,
    hooks: Option<UserMod>,
    /// `before` in hooks.rs is an `async fn`: every request may wait.
    before_waits: bool,
    /// The app's own modules (`src/notes.rs`).
    mods: Vec<UserMod>,
    /// The types of `src/*.rs`, which endpoints and action forms may name.
    shared: Vec<rust_scan::TypeItem>,
}

/// The browser's half: the modules of templates (by template), and the
/// other JavaScript files served.
struct Web {
    clients: Vec<Option<Client>>,
    js_files: Vec<JsFile>,
}

impl<'a> Project<'a> {
    fn new(input: &Input<'a>) -> Result<Project<'a>, String> {
        let root = input.root;
        let tree = crate::routes::scan(&root.join("src").join("routes"))?;
        let shell_path = root.join("src").join("app.html");
        let shell_src = match shell_path.exists() {
            true => crate::read_source(&shell_path).map_err(|e| format!("src/app.html: {e}"))?,
            false => shell::DEFAULT.to_string(),
        };
        let shell = shell::split(&shell_src).map_err(|e| format!("src/app.html: {e}"))?;
        Ok(Project {
            root,
            release: input.release,
            tree,
            shell,
            comps: Vec::new(),
            templates: Vec::new(),
            user_mods: Vec::new(),
            model: Model::default(),
            hooks: None,
            before_waits: false,
            mods: Vec::new(),
            shared: crate::shared_types(root),
        })
    }

    /// The whole app, read phase by phase, each finding what the next
    /// needs, into its model.
    fn load(input: &Input<'a>) -> Result<Project<'a>, String> {
        let mut p = Project::new(input)?;
        p.components()?;
        p.layouts()?;
        p.error_pages()?;
        p.routes()?;
        p.app_files()?;
        Ok(p)
    }

    /// `p` from the project root, `/`-separated: how errors name a file.
    fn rel(&self, p: &Path) -> String {
        p.strip_prefix(self.root)
            .unwrap_or(p)
            .to_string_lossy()
            .replace('\\', "/")
    }

    fn read(&self, p: &Path) -> Result<String, String> {
        crate::read_source(p).map_err(|e| format!("{}: {e}", p.display()))
    }

    /// A layout or error page: with the Rust of its `---` block, if it has
    /// one.
    fn parse(&self, p: &Path) -> Result<(Template, Option<String>), String> {
        let (front, markup) =
            crate::split_front(&self.read(p)?).map_err(|e| format!("{}:{e}", self.rel(p)))?;
        self.markup(p, &markup, front, &[])
    }

    /// The markup of `p`, its action forms' `fields` given the browser's
    /// checks: everything but a component.
    fn markup(
        &self,
        p: &Path,
        markup: &str,
        front: Option<String>,
        fields: &[rules::Field],
    ) -> Result<(Template, Option<String>), String> {
        let at = |e: String| format!("{}:{e}", self.rel(p));
        let (t, rust) = crate::parse_markup(markup, front, fields).map_err(at)?;
        if let Some((_, line)) = t.props {
            return Err(at(format!(
                "{line}: only components, in src/components, take props"
            )));
        }
        Ok((t, rust))
    }

    /// A `+layout.rs`, `+page.rs` or `+server.rs`.
    fn scan(&self, p: &Path) -> Result<rust_scan::Items, String> {
        let at = |e: String| format!("{}:{e}", self.rel(p));
        let items = rust_scan::scan(&self.read(p)?).map_err(at)?;
        items.check().map_err(at)?;
        Ok(items)
    }

    /// A page's or layout's Rust: its `+page.rs` or `+layout.rs` (`rs`), or
    /// the `---` block of its `.wisp`, split into the items its module holds
    /// and the statements that run before it renders.
    fn logic(
        &self,
        rs: Option<PathBuf>,
        wisp: &Path,
        front: Option<String>,
    ) -> Result<Logic, String> {
        let Some(code) = front else {
            let items = match &rs {
                Some(f) => self.scan(f)?,
                None => rust_scan::Items::default(),
            };
            return Ok(Logic {
                items,
                file: rs,
                inline: None,
                stmts: None,
            });
        };
        if let Some(f) = rs {
            return Err(format!(
                "{}: this file starts with a `---` block of Rust, and {} is beside it; keep the Rust in one of them",
                self.rel(wisp),
                f.file_name()
                    .map(|n| n.to_string_lossy())
                    .unwrap_or_default()
            ));
        }
        let (items_src, stmts) = rust_scan::split_items(&code);
        let at = |e: String| format!("{}:{e}", self.rel(wisp));
        let items = rust_scan::scan(&items_src).map_err(at)?;
        items.check().map_err(at)?;
        let stmts = (!stmts.trim().is_empty()).then_some(stmts);
        if let (Some(s), Some(load)) = (&stmts, items.function("load")) {
            let first = s.lines().position(|l| !l.trim().is_empty()).unwrap_or(0) + 1;
            return Err(at(format!(
                "{first}: the statements of a `---` block are the page's load, and this block also has `fn load` (line {}); keep one",
                load.line
            )));
        }
        Ok(Logic {
            items,
            file: Some(wisp.to_path_buf()),
            inline: Some(items_src),
            stmts,
        })
    }

    /// Adds the template `t` of `file` (its module `module`), which is for
    /// `kind`; the rest of what it has is the caller's to fill in.
    fn add_tpl(&mut self, module: String, file: &Path, kind: Kind, t: Template) -> &mut Tpl {
        let rel = self.rel(file);
        self.templates.push(Tpl {
            id: self.templates.len() + 1,
            module,
            rel,
            kind,
            user: None,
            data: Vec::new(),
            load_js: None,
            stmts: None,
            t,
        });
        self.templates.last_mut().expect("just pushed")
    }

    /// `src/components/*.wisp`: templates `1..=N`, one per component.
    fn components(&mut self) -> Result<(), String> {
        let comp_dir = self.root.join("src").join("components");
        if !comp_dir.is_dir() {
            return Ok(());
        }
        let mut files = Vec::new();
        list_files(&comp_dir, &mut files)?;
        files.sort();
        for file in files {
            let file_name = file
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            if crate::routes::editor_temp(&file_name)
                || file.extension().is_none_or(|e| e != "wisp")
            {
                continue;
            }
            let rel = self.rel(&file);
            let name = file_name.trim_end_matches(".wisp").to_string();
            if !template::is_component_name(&name) {
                // `card.wisp` → `Card.wisp`, `UI.wisp` → `Ui.wisp`.
                let clean: String = name
                    .chars()
                    .filter(|c| c.is_ascii() && ty::is_word(*c as u8))
                    .collect();
                let lower = clean.bytes().any(|b| b.is_ascii_lowercase());
                let suggest: String = clean
                    .chars()
                    .enumerate()
                    .map(|(i, c)| match i {
                        0 => c.to_ascii_uppercase(),
                        _ if lower => c,
                        _ => c.to_ascii_lowercase(),
                    })
                    .collect();
                return Err(format!(
                    "{rel}: a component's file name is its tag: a capital letter first, lowercase letters too (a tag in all capitals is HTML), \
                     and only letters, digits and _, such as {suggest}.wisp"
                ));
            }
            if let Some(other) = self.comps.iter().find(|c| c.name == name) {
                return Err(format!(
                    "{rel}: there is already a component `{name}` ({})",
                    other.module
                ));
            }
            let (mut t, rust) =
                crate::parse_wisp(&self.read(&file)?).map_err(|e| format!("{rel}:{e}"))?;
            if rust.is_some() {
                return Err(format!(
                    "{rel}: a component takes what it shows as {{@props …}}; a `---` block of Rust is for pages and layouts"
                ));
            }
            if let Some(line) = first_await(&t.nodes) {
                return Err(format!(
                    "{rel}:{line}: a component renders without waiting, so its markup cannot `.await`; await in the page and pass the value as a prop"
                ));
            }
            let rune = match &t.script {
                Some(s) => js::props_rune(&s.src).map_err(|(off, msg)| {
                    let (line, col) = script_pos(s, off);
                    format!("{rel}:{line}:{col}: {msg}")
                })?,
                None => None,
            };
            // Without {@props}, `$props()` says what the component takes.
            if t.props.is_none()
                && let Some(r) = &rune
            {
                t.props = Some((inferred_props(r), 0));
            }
            let props = t.props.as_ref().map(|(p, _)| p.clone()).unwrap_or_default();
            let module = format!("tpl_component_{}", self.comps.len());
            let bindable = rune.as_ref().map(|p| {
                p.props
                    .iter()
                    .filter(|x| x.bindable)
                    .map(|x| x.name.clone())
                    .collect()
            });
            self.comps.push(Comp {
                name,
                module: module.clone(),
                rest: props.iter().any(|d| d.name == REST),
                props,
                children: t.uses_children,
                bindable,
                live: t.is_live(),
            });
            self.add_tpl(module, &file, Kind::Component, t);
        }
        Ok(())
    }

    fn layouts(&mut self) -> Result<(), String> {
        for i in 0..self.tree.layouts.len() {
            let dir = self.tree.layouts[i].dir.clone();
            let file = dir.join("+layout.wisp");
            let (t, front) = self.parse(&file)?;
            if let Some(line) = first_await(&t.nodes) {
                return Err(format!(
                    "{}:{line}: a layout renders without waiting, so its markup cannot `.await`; await in the page's markup or `---` block",
                    self.rel(&file)
                ));
            }
            if !t.uses_children {
                return Err(format!(
                    "{}: a layout must contain {{@render children()}}",
                    self.rel(&file)
                ));
            }
            let rs = self.tree.layouts[i].has_rs.then(|| dir.join("+layout.rs"));
            let lg = self.logic(rs, &file, front)?;
            let where_ = lg.file.as_deref().map(|f| self.rel(f)).unwrap_or_default();
            if let Some(a) = lg.items.fns.iter().find(|f| f.action) {
                return Err(format!(
                    "{where_}:{}: layouts cannot have actions (`{}`); put it in the page",
                    a.line, a.name
                ));
            }
            if let Some(c) = lg.items.constant("BODY_LIMIT") {
                return Err(format!(
                    "{where_}:{}: a layout's `BODY_LIMIT` does nothing; set it in the page or +server.rs whose requests it limits",
                    c.line
                ));
            }
            if let Some(c) = ["CACHE", "CACHE_PUBLIC"]
                .iter()
                .find_map(|n| lg.items.constant(n))
            {
                return Err(format!(
                    "{where_}:{}: a layout's `{}` does nothing; set it in the page or +server.rs whose responses it keeps",
                    c.line, c.name
                ));
            }
            let load = lg.items.function("load");
            // The template reads `data` from a load, or names from statements.
            let reads = load.is_some() || lg.stmts.is_some();
            let user = lg.file.is_some().then(|| (format!("layout_{i}"), reads));
            // Any `async fn`, shim or helper, may be waited on: a mistake
            // errs towards waiting (see `now`).
            let waits = lg.file.is_some() && lg.items.fns.iter().any(|f| f.is_async);
            if let Some(src) = lg.file {
                let shims = load
                    .map(|f| shim(f, Shim::Load))
                    .transpose()
                    .map_err(|e| format!("{where_}:{e}"))?;
                let name = format!("layout_{i}");
                let shims = shims.into_iter().collect();
                self.user_mods
                    .push(UserMod::new(name, src, lg.inline, shims, &lg.items));
            }
            let load = load.is_some();
            let data = lg.items.data_fields();
            let tpl = self.add_tpl(format!("tpl_layout_{i}"), &file, Kind::Layout, t);
            tpl.user = user;
            tpl.data = data;
            tpl.stmts = lg.stmts.map(|s| (s, Vec::new()));
            let tpl = self.templates.len() - 1;
            self.model.layouts.push(model::Layout { tpl, load, waits });
        }
        Ok(())
    }

    fn error_pages(&mut self) -> Result<(), String> {
        let routes_dir = self.root.join("src").join("routes");
        for i in 0..self.tree.errors.len() {
            let dir = &self.tree.errors[i].dir;
            if *dir == routes_dir {
                self.model.root_error = Some(i);
            }
            let file = dir.join("+error.wisp");
            let (t, front) = self.parse(&file)?;
            check_no_children(&t, &self.rel(&file))?;
            if front.is_some() {
                return Err(format!(
                    "{}: an error page shows `status` and `message` (and can read `cx`); a `---` block of Rust is for pages and layouts",
                    self.rel(&file)
                ));
            }
            self.add_tpl(format!("tpl_error_{i}"), &file, Kind::Error, t);
            self.model.errors.push(model::ErrorPage {
                tpl: self.templates.len() - 1,
                layouts: self.tree.errors[i].layouts.clone(),
            });
        }
        Ok(())
    }

    /// A route's `BODY_LIMIT`, checked: a `usize`, set once.
    fn body_limit(
        &self,
        route: &mut model::Route,
        items: &rust_scan::Items,
        file: &Path,
        module: String,
        page: &str,
        shims: &mut Vec<String>,
    ) -> Result<(), String> {
        let Some(c) = items.constant("BODY_LIMIT") else {
            return Ok(());
        };
        let at = |msg: &str| format!("{}:{}: {msg}", self.rel(file), c.line);
        if c.ty != "usize" {
            return Err(at(&format!(
                "`BODY_LIMIT` is a `{}`; make it a `usize`, such as `20 * wisp::MB`",
                c.ty
            )));
        }
        set_once(&mut route.body_limit, module, "BODY_LIMIT", page).map_err(|e| at(&e))?;
        shims.push("pub const BODY_LIMIT: usize = super::BODY_LIMIT;".into());
        Ok(())
    }

    /// A route's `CACHE` (or `CACHE_PUBLIC`), checked: a `u32`, one of the
    /// two, set once.
    fn cache(
        &self,
        route: &mut model::Route,
        items: &rust_scan::Items,
        file: &Path,
        module: String,
        page: &str,
        shims: &mut Vec<String>,
    ) -> Result<(), String> {
        let (c, public) = match (items.constant("CACHE"), items.constant("CACHE_PUBLIC")) {
            (None, None) => return Ok(()),
            (Some(c), None) => (c, false),
            (None, Some(c)) => (c, true),
            (Some(_), Some(c)) => {
                return Err(format!(
                    "{}:{}: `CACHE_PUBLIC` is `CACHE` shared with signed-in visitors too; set one of them",
                    self.rel(file),
                    c.line
                ));
            }
        };
        let at = |msg: &str| format!("{}:{}: {msg}", self.rel(file), c.line);
        if c.ty != "u32" || c.is_static {
            return Err(at(&format!(
                "`{}` is a `{}`; make it a `const` `u32`, the seconds a response is kept, such as `const {0}: u32 = 60;`",
                c.name, c.ty
            )));
        }
        let cache = model::Cache { module, public };
        set_once(&mut route.cache, cache, &c.name, page).map_err(|e| at(&e))?;
        shims.push(format!("pub const CACHE: u32 = super::{};", c.name));
        Ok(())
    }

    /// Pages and endpoints.
    fn routes(&mut self) -> Result<(), String> {
        // Each `+server.rs` once, though it may serve two routes.
        let mut servers: Vec<ServerFile> = Vec::new();
        for i in 0..self.tree.routes.len() {
            let r = &self.tree.routes[i];
            let error = r.error;
            let mut route = model::Route {
                pattern: r.pattern(),
                page: None,
                server: None,
                layouts: r.layouts.clone(),
                error,
                body_limit: None,
                uploads: None,
                cache: None,
            };
            if r.page {
                self.page(i, &mut route)?;
            }
            if self.tree.routes[i].server {
                self.server(i, &mut route, &mut servers)?;
            }
            self.model.routes.push(route);
        }
        Ok(())
    }

    /// The `+page.wisp` of route `i`, and its Rust.
    fn page(&mut self, i: usize, route: &mut model::Route) -> Result<(), String> {
        let r = &self.tree.routes[i];
        let (dir, page_rs, page_js) = (r.dir.clone(), r.page_rs, r.page_js);
        let file = dir.join("+page.wisp");
        // Its Rust first: its actions' fields get the browser's checks.
        let src = self.read(&file)?;
        let (front, markup) =
            crate::split_front(&src).map_err(|e| format!("{}:{e}", self.rel(&file)))?;
        let mut lg = self.logic(page_rs.then(|| dir.join("+page.rs")), &file, front.clone())?;
        let fields = rules::fields(&lg.items, &self.tree.routes[i].params(), &self.shared);
        let (mut t, _) = self.markup(&file, &markup, front, &fields)?;
        check_no_children(&t, &self.rel(&file))?;
        // `.await` in the markup: statements, after the block's own.
        let mut lets = Vec::new();
        hoist_awaits(&mut t.nodes, &mut lets).map_err(|e| format!("{}:{e}", self.rel(&file)))?;
        if let (Some((_, line)), Some(load)) = (lets.first(), lg.items.function("load")) {
            return Err(format!(
                "{}:{line}: `.await` in markup runs with the page's statements, and this page has `fn load` (line {}); await in `load`",
                self.rel(&file),
                load.line
            ));
        }
        lg.stmts = with_lets(lg.stmts, &lets);
        let rs = lg.file.clone().unwrap_or_else(|| dir.join("+page.rs"));
        let mut shims = Vec::new();
        // The page comes first: nothing set these before it.
        self.body_limit(route, &lg.items, &rs, format!("page_{i}"), "", &mut shims)?;
        self.cache(route, &lg.items, &rs, format!("page_{i}"), "", &mut shims)?;
        if let Some(sum) =
            upload_sizes(&lg.items.fns).map_err(|e| format!("{}:{e}", self.rel(&rs)))?
        {
            route.uploads = Some(format!("page_{i}"));
            shims.push(format!("pub const UPLOADS: usize = {sum};"));
        }
        let data = lg.items.data_fields();
        let tables = lg.items.tables();
        let fns = lg.items.fns;
        let has_load = fns.iter().any(|f| f.name == "load");
        if lg.stmts.is_some() && page_js {
            return Err(format!(
                "{}: +page.js gets the page's `data`, which comes from a `load`; with a `---` block of statements there is none. Move them into `fn load`.",
                self.rel(&file)
            ));
        }
        // A `let` for each route parameter the statements or markup name.
        let binds: Vec<String> = self.tree.routes[i]
            .segs
            .iter()
            .filter_map(|s| {
                let (n, how) = match s {
                    Seg::Static(_) => return None,
                    Seg::Param(n, m) if m.as_deref() == Some("int") => {
                        (n, ": u64 = cx.param(@).parse().unwrap_or_default()")
                    }
                    Seg::Param(n, _) | Seg::Rest(n) => (n, " = cx.param(@).to_string()"),
                    Seg::Optional(n, m) if m.as_deref() == Some("int") => {
                        (n, ": Option<u64> = cx.param(@).parse().ok()")
                    }
                    Seg::Optional(n, _) => (
                        n,
                        ": Option<String> = Some(cx.param(@)).filter(|s| !s.is_empty()).map(str::to_string)",
                    ),
                };
                names_word(&src, n).then(|| format!("let {n}{};", how.replace('@', &lit(n))))
            })
            .collect();
        let at = |f: &FnItem, msg: String| format!("{}:{}: {msg}", self.rel(&rs), f.line);
        if let Some(f) = fns.iter().find(|f| f.action && f.name == "load") {
            return Err(at(
                f,
                format!("`{}` cannot be both load and an action", f.name),
            ));
        }
        if let Some(f) = fns.iter().find(|f| f.name == "entries") {
            if !f.params.is_empty() || f.is_async || f.action {
                return Err(at(
                    f,
                    "`entries` is `fn entries() -> Vec<...>`: no parameters, not async, not an action".into(),
                ));
            }
            shims.push("pub fn entries() -> Vec<Vec<String>> { super::entries().into_iter().map(::wisp::Entry::params).collect() }".into());
        }
        if let Some(f) = fns
            .iter()
            .find(|f| f.action && f.returns_kind() == Returns::Other)
        {
            return Err(at(
                f,
                format!(
                    "action `{}` returns `{}`. An action returns nothing (or `Result<()>`, so it can use `?`), \
                     or a `Response` to send instead of the page (or `Option<Response>`, to send one only sometimes).",
                    f.name, f.returns
                ),
            ));
        }
        for f in &fns {
            let kind = match f.name.as_str() {
                _ if f.action => Shim::Answer,
                "load" => Shim::Load,
                _ => continue,
            };
            shims.push(shim(f, kind).map_err(|e| format!("{}:{e}", self.rel(&rs)))?);
        }
        let reads = has_load || lg.stmts.is_some();
        let user = lg.file.is_some().then(|| (format!("page_{i}"), reads));
        if lg.file.is_some() {
            self.user_mods.push(UserMod {
                name: format!("page_{i}"),
                file: rs,
                inline: lg.inline,
                shims,
                tables,
            });
        }
        let tpl = self.add_tpl(format!("tpl_page_{i}"), &file, Kind::Page, t);
        tpl.user = user;
        tpl.data = data;
        tpl.load_js = page_js.then(|| dir.join("+page.js"));
        let waits =
            fns.iter().any(|f| f.is_async) || lg.stmts.as_deref().is_some_and(rust_scan::may_wait);
        // A page with no Rust still reads its route parameters.
        tpl.stmts = match lg.stmts {
            Some(s) => Some((s, binds)),
            None if !has_load && !binds.is_empty() => Some((String::new(), binds)),
            None => None,
        };
        route.page = Some(model::Page {
            module: format!("page_{i}"),
            tpl: self.templates.len() - 1,
            fns,
            waits,
        });
        Ok(())
    }

    /// The `+server.rs` of route `i`, read once for the two routes it may
    /// serve (`servers` has those read so far).
    fn server(
        &mut self,
        i: usize,
        route: &mut model::Route,
        servers: &mut Vec<ServerFile>,
    ) -> Result<(), String> {
        let r = &self.tree.routes[i];
        let (page, member) = (r.page, r.member);
        let page_file = if r.page_rs { "+page.rs" } else { "+page.wisp" };
        let file = r.dir.join("+server.rs");
        let k = match servers.iter().position(|s| s.file == file) {
            Some(k) => {
                let (sf, at) = (&servers[k], |e: String| format!("{}: {e}", self.rel(&file)));
                if sf.limit {
                    set_once(
                        &mut route.body_limit,
                        sf.server.module.clone(),
                        "BODY_LIMIT",
                        page_file,
                    )
                    .map_err(at)?;
                }
                if let Some(public) = sf.cache {
                    let cache = model::Cache {
                        module: sf.server.module.clone(),
                        public,
                    };
                    set_once(&mut route.cache, cache, "CACHE", page_file).map_err(at)?;
                }
                k
            }
            None => {
                let items = self.scan(&file)?;
                let module = format!("server_{i}");
                let mut shims = Vec::new();
                self.body_limit(route, &items, &file, module.clone(), page_file, &mut shims)?;
                self.cache(route, &items, &file, module.clone(), page_file, &mut shims)?;
                let cache = (route.cache.as_ref())
                    .filter(|c| c.module == module)
                    .map(|c| c.public);
                let segs = &self.tree.routes[i].segs;
                let segs = &segs[..segs.len() - usize::from(member)];
                let (handlers, before) = server_handlers(&items, segs, &mut shims)
                    .map_err(|e| format!("{}:{e}", self.rel(&file)))?;
                let waits = items.fns.iter().any(|f| f.is_async);
                (self.user_mods).push(UserMod::new(
                    module.clone(),
                    file.clone(),
                    None,
                    shims,
                    &items,
                ));
                servers.push(ServerFile {
                    file: file.clone(),
                    limit: route.body_limit.as_ref() == Some(&module),
                    cache,
                    server: model::Server {
                        module,
                        handlers,
                        before,
                        types: items.types.into(),
                        waits,
                    },
                });
                servers.len() - 1
            }
        };
        let sf = &servers[k].server;
        let handlers: Vec<model::Handler> = (sf.handlers.iter())
            .filter(|h| h.member == member)
            .cloned()
            .collect();
        let has_actions = route
            .page
            .as_ref()
            .is_some_and(|p| p.actions().next().is_some());
        for h in &handlers {
            if page && (h.op.method == "get" || (h.op.method == "post" && has_actions)) {
                return Err(format!(
                    "{}: `{}` conflicts with the page in the same directory",
                    self.rel(&file),
                    h.shim
                ));
            }
        }
        route.server = Some(model::Server {
            module: sf.module.clone(),
            handlers,
            before: sf.before,
            types: sf.types.clone(),
            waits: sf.waits,
        });
        Ok(())
    }

    /// `src/hooks.rs`, the param matchers, the app's own modules; then,
    /// with every template read, that the components they use exist.
    fn app_files(&mut self) -> Result<(), String> {
        (self.hooks, self.before_waits) = hooks(self.root)?;
        for (m, file) in &self.tree.matchers {
            let Some(file) = file else { continue };
            let at = |e: String| format!("{}:{e}", self.rel(file));
            let items = rust_scan::scan(&self.read(file)?).map_err(at)?;
            items.check_inner().map_err(at)?;
            if items.function("matches").is_none() {
                return Err(format!(
                    "{}: a param matcher is `fn matches(s: &str) -> bool`, which this file does not have",
                    self.rel(file)
                ));
            }
            let shims = vec!["pub fn matches(s: &str) -> bool { super::matches(s) }".into()];
            let name = format!("param_{m}");
            self.user_mods
                .push(UserMod::new(name, file.clone(), None, shims, &items));
        }
        self.mods = app_mods(self.root)?;
        for t in &self.templates {
            check_components(&t.t.nodes, &t.t, &self.comps, &t.rel, false)?;
        }
        Ok(())
    }

    fn has_hook(&self, name: &str) -> bool {
        self.hooks.as_ref().is_some_and(|h| h.has(name))
    }

    /// Browser JavaScript: `src/lib/**/*.js`, imported as `$lib/…` and
    /// served under one hash (so every importer names a file by the same
    /// URL), each page's `+page.js`, and the modules of templates.
    fn browser(&self) -> Result<Web, String> {
        let mut lib = Vec::new();
        let lib_dir = self.root.join("src").join("lib");
        if lib_dir.is_dir() {
            list_files(&lib_dir, &mut lib)?;
        }
        lib.retain(|f| f.extension().is_some_and(|e| e == "js"));
        lib.sort();
        let mut lib_src = Vec::new();
        for f in &lib {
            let path = f
                .strip_prefix(&lib_dir)
                .unwrap_or(f)
                .to_string_lossy()
                .replace('\\', "/");
            lib_src.push((path, self.read(f)?));
        }
        let lib_hash = {
            let mut h = Vec::new();
            for (p, src) in &lib_src {
                h.extend_from_slice(p.as_bytes());
                h.push(0);
                h.extend_from_slice(src.as_bytes());
                h.push(0);
            }
            format!("{:016x}", fnv1a(&h))
        };
        // The runtime's less used half, which modules that use it import.
        let extra = {
            let src = rewrite_specifiers(EXTRA_JS, &lib_hash, None);
            let source = if self.release { js::runtime(&src) } else { src };
            let hash = format!("{:016x}", fnv1a(source.as_bytes()));
            JsFile {
                path: EXTRA_JS_PATH.into(),
                hash,
                source,
            }
        };
        let extra_url = format!("{}?v={}", extra.path, extra.hash);
        // A lib file that makes a `persisted` store imports it too (last, so
        // its lines stay).
        let lib_file = |src: &str, dir: Option<&str>| {
            let mut s = rewrite_specifiers(src, &lib_hash, dir);
            if js::tokens(src)
                .iter()
                .any(|t| !t.member && t.text(src) == "persisted")
            {
                s.push_str(&format!("\nimport {};\n", js_str(&extra_url)));
            }
            s
        };
        let mut js_files: Vec<JsFile> = lib_src
            .iter()
            .map(|(p, src)| JsFile {
                path: format!("{MODULES}lib/{p}"),
                hash: lib_hash.clone(),
                source: lib_file(src, Some(p.rfind('/').map_or("", |i| &p[..i]))),
            })
            .collect();

        // The modules of templates.
        let as_client: std::collections::HashSet<String> = self
            .templates
            .iter()
            .flat_map(|t| client_uses(&t.t))
            .collect();
        let mut clients: Vec<Option<Client>> = Vec::with_capacity(self.templates.len());
        for (k, t) in self.templates.iter().enumerate() {
            let load = match &t.load_js {
                Some(f) => {
                    let source = lib_file(&self.read(f)?, None);
                    let hash = format!("{:016x}", fnv1a(source.as_bytes()));
                    let path = format!("{MODULES}t{}.load.js", t.id);
                    let url = format!("{path}?v={hash}");
                    js_files.push(JsFile { path, hash, source });
                    Some(url)
                }
                None => None,
            };
            // Components are the first templates.
            let is_client = t.kind == Kind::Component && as_client.contains(&self.comps[k].name);
            let cx = ClientCx {
                comps: &self.comps,
                templates: &self.templates,
                as_client: is_client,
                lib_hash: &lib_hash,
                load,
                release: self.release,
                extra: &extra_url,
            };
            clients.push(client(t, &cx)?);
        }
        if clients
            .iter()
            .flatten()
            .any(|c| c.source.contains(&extra_url))
        {
            js_files.push(extra);
        }
        // A module imports the modules of the components it renders by URLs
        // whose hash covers every module it can reach, so a change in any of
        // them changes the URL. Components may render each other (or
        // themselves) in a circle: a hash over the set needs no order.
        let finals: Vec<String> = (0..clients.len())
            .map(|k| {
                let mut seen = vec![false; clients.len()];
                seen[k] = true;
                let mut stack: Vec<usize> =
                    clients[k].as_ref().map_or(Vec::new(), |c| c.uses.clone());
                while let Some(ci) = stack.pop() {
                    if !std::mem::replace(&mut seen[ci], true) {
                        stack.extend(clients[ci].iter().flat_map(|c| &c.uses));
                    }
                }
                let all: String = (0..clients.len())
                    .filter(|&j| seen[j])
                    .filter_map(|j| clients[j].as_ref().map(|c| c.hash.as_str()))
                    .collect();
                format!("{:016x}", fnv1a(all.as_bytes()))
            })
            .collect();
        for (k, c) in clients.iter_mut().enumerate() {
            let Some(c) = c else { continue };
            let url = |ci: usize| {
                c.uses
                    .contains(&ci)
                    .then(|| format!("{MODULES}t{}.js?v={}", self.templates[ci].id, finals[ci]))
            };
            c.source = link_comps(&c.source, url);
            c.hash.clone_from(&finals[k]);
        }
        Ok(Web { clients, js_files })
    }

    /// Per route, its page when it is the same for every request, whole, as
    /// it is sent: the shell with the head tags `setup` in wisp's http.rs
    /// makes out of dev mode (`css` is `App::CSS`), the page's head and its
    /// body. That is a page and layouts with no Rust that reads anything (a
    /// load, statements, a `+page.js`) and markup the build can write out
    /// (see `fold`).
    fn baked(&self, css: Option<&str>) -> Vec<Option<String>> {
        let comp = |name: &str| {
            let k = self.comps.iter().position(|c| c.name == name)?;
            let c = &self.comps[k];
            (!c.live).then(|| (&self.templates[k].t, &c.props[..]))
        };
        let fold = fold::Fold { comp: &comp };
        let reads = |t: &Tpl| {
            t.stmts.is_some() || t.load_js.is_some() || t.user.as_ref().is_some_and(|(_, r)| *r)
        };
        let mut tags = String::new();
        if let Some(v) = css {
            let _ = write!(
                tags,
                "<link rel=\"stylesheet\" href=\"{APP_CSS_PATH}?v={v}\">"
            );
        }
        let _ = write!(
            tags,
            "<script defer src=\"{WISP_JS_PATH}?v={}\"></script>",
            crate::runtime_version()
        );
        let [s0, s1, s2] = &self.shell;
        let m = &self.model;
        (m.routes.iter())
            .map(|r| {
                let tpl = |&l: &usize| &self.templates[m.layouts[l].tpl];
                let mut layers: Vec<&Tpl> = r.layouts.iter().map(tpl).collect();
                layers.push(&self.templates[r.page.as_ref()?.tpl]);
                if layers.iter().any(|t| reads(t)) {
                    return None;
                }
                let ts: Vec<&Template> = layers.iter().map(|t| &t.t).collect();
                let doc = fold.page(&ts)?;
                Some(
                    [s0, &tags, &doc.head, s1, &doc.body, s2]
                        .map(String::as_str)
                        .concat(),
                )
            })
            .collect()
    }

    /// `layout_0::tpl_layout_0::render(__o, &d0, &|__o| tpl_layout_3::render(__o, &|__o| inner))`:
    /// `inner` inside `layouts`.
    fn wrap_layouts(&self, layouts: &[usize], inner: String) -> String {
        layouts.iter().rev().fold(inner, |acc, &l| {
            let layout = &self.model.layouts[l];
            let data = if layout.load {
                format!(", &d{l}")
            } else {
                String::new()
            };
            format!(
                "{}::render(__o, cx{data}, &|__o: &mut ::wisp::Out| {acc})",
                self.templates[layout.tpl].path()
            )
        })
    }
}

impl Gen {
    /// The header, then the app's own Rust files, each included into a
    /// module with the prelude, its shims, and the templates it gives data
    /// to; then the shell, the other templates and the JavaScript files.
    fn modules(&mut self, p: &Project, web: &Web) -> Result<(), String> {
        self.line(
            0,
            &format!(
                "// @generated by wisp-build for {}. Do not edit.",
                p.root.display()
            ),
        );
        self.line(0, "");
        // Browser modules import the runtime by the version wisp-build gives it.
        self.line(0, &format!(
            "const _: () = assert!(::wisp::rt::same_version({}), \"wisp and wisp-build are different versions: use the same version of both\");",
            lit(crate::runtime_version())
        ));
        self.line(0, "");
        if p.hooks.is_none() {
            self.line(0, "pub mod hooks {}");
        }
        // Modules of the app's own (`src/notes.rs`), reachable by name from
        // every route file and as `crate::notes` (see `wisp::app!`).
        self.line(0, "#[doc(hidden)]");
        self.line(0, "pub mod __mods {");
        for m in &p.mods {
            self.user_mod(m, &p.rel(&m.file), "super::*")?;
            if !m.tables.is_empty() {
                self.call_mod(m, &[]);
            }
            self.line(0, "}");
        }
        self.line(0, "}");
        self.line(0, "");
        // The sync twins `handle_now` calls, as `module::shim`.
        let twins: Vec<String> = (p.model.routes.iter())
            .flat_map(|r| now_arms(p, r))
            .map(|(s, h)| format!("{}::{}", s.module, h.shim))
            .collect();
        for m in p.hooks.iter().chain(&p.user_mods) {
            self.user_mod(m, &p.rel(&m.file), "super::__mods::*")?;
            self.call_mod(m, &twins);
            for (t, c) in p.templates.iter().zip(&web.clients) {
                if t.user.as_ref().is_some_and(|(u, _)| *u == m.name) {
                    self.template(t, &p.comps, c.as_ref());
                }
            }
            self.line(0, "}");
            self.line(0, "");
        }

        self.line(0, "#[doc(hidden)]");
        self.line(0, "pub mod tpl_shell {");
        let [a, b, c] = &p.shell;
        self.line(
            1,
            &format!(
                "pub static S: [&str; 3] = [{}, {}, {}];",
                lit(a),
                lit(b),
                lit(c)
            ),
        );
        self.line(0, "}");
        self.line(0, "");

        for (i, f) in web.js_files.iter().enumerate() {
            self.line(0, &format!(
                "static __WISP_JS_{i}: ::wisp::ClientModule = ::wisp::ClientModule {{ id: {}, path: {}, url: {}, etag: {}, source: {}, preload: \"\" }};",
                lit(&f.path),
                lit(&f.path),
                lit(&format!("{}?v={}", f.path, f.hash)),
                lit(&format!("\"{}\"", f.hash)),
                lit(&f.source)
            ));
        }
        for (t, c) in p.templates.iter().zip(&web.clients) {
            if t.user.is_none() {
                self.template(t, &p.comps, c.as_ref());
            }
        }
        Ok(())
    }

    /// A function per page, which runs the loads outermost first and then
    /// renders inside the layouts, and per error page.
    fn servers(&mut self, p: &Project) {
        let m = &p.model;
        let layout_load = |l: &&usize| m.layouts[**l].load;
        for (i, (r, route)) in p.tree.routes.iter().zip(&m.routes).enumerate() {
            let Some(pg) = &route.page else {
                continue;
            };
            let page = &p.templates[pg.tpl];
            self.line(0, "#[allow(unused_variables)]");
            self.line(0, &format!("async fn serve_page_{i}(cx: &mut ::wisp::Cx, __o: &mut ::wisp::Out) -> ::wisp::Result<()> {{"));
            for l in route.layouts.iter().filter(layout_load) {
                self.line(
                    1,
                    &format!("let d{l} = layout_{l}::__call::load(cx).await?;"),
                );
            }
            let page_load = pg.load();
            if page_load {
                self.line(1, &format!("let d = page_{i}::__call::load(cx).await?;"));
            }
            // A `+page.js` load gets the route and its parameters.
            if page.load_js.is_some() {
                let params: Vec<String> = r
                    .params()
                    .iter()
                    .map(|p| format!("({0}, cx.param({0}))", lit(p)))
                    .collect();
                self.line(
                    1,
                    &format!(
                        "::wisp::rt::live_route(__o, {}, &[{}]);",
                        lit(&r.pattern()),
                        params.join(", ")
                    ),
                );
            }
            if page.stmts.is_some() {
                // The page runs its statements, then hands its render to this
                // closure, which puts it inside the layouts.
                let wrap = p.wrap_layouts(&route.layouts, "__p(__o)".into());
                self.line(1, &format!(
                    "{}::render(cx, __o, |__o: &mut ::wisp::Out, cx: &::wisp::Cx, __p: &dyn Fn(&mut ::wisp::Out)| {wrap}).await",
                    page.path()
                ));
            } else {
                let data = if page_load { ", &d" } else { "" };
                let inner = format!("{}::render(__o, cx{data})", page.path());
                self.line(1, "let cx: &::wisp::Cx = cx;");
                self.line(1, &format!("{};", p.wrap_layouts(&route.layouts, inner)));
                self.line(1, "Ok(())");
            }
            self.line(0, "}");
            self.line(0, "");
        }

        // One per +error.wisp, inside the layouts of its directory.
        for (i, e) in m.errors.iter().enumerate() {
            self.line(0, "#[allow(unused_variables)]");
            self.line(0, &format!("async fn serve_error_{i}(cx: &mut ::wisp::Cx, __o: &mut ::wisp::Out, status: u16, message: &str) -> ::wisp::Result<()> {{"));
            for l in e.layouts.iter().filter(layout_load) {
                self.line(
                    1,
                    &format!("let d{l} = layout_{l}::__call::load(cx).await?;"),
                );
            }
            let inner = format!(
                "{}::render(__o, cx, status, message)",
                p.templates[e.tpl].path()
            );
            self.line(1, "let cx: &::wisp::Cx = cx;");
            self.line(1, &format!("{};", p.wrap_layouts(&e.layouts, inner)));
            self.line(1, "Ok(())");
            self.line(0, "}");
            self.line(0, "");
        }
    }

    /// The CSS, and in a release build the static files, embedded.
    fn assets(&mut self, p: &Project) -> Result<Assets, String> {
        let css = css_source(p.root)?;
        let mut files: Vec<(String, PathBuf, String)> = Vec::new();
        let css_hash = match &css {
            Some(f) => Some(format!(
                "{:016x}",
                fnv1a(&fs::read(f).map_err(|e| format!("{}: {e}", f.display()))?)
            )),
            None => None,
        };
        if !p.release {
            return Ok(Assets { css_hash, files });
        }
        if let (Some(f), Some(h)) = (&css, &css_hash) {
            files.push((APP_CSS_PATH.into(), f.clone(), h.clone()));
        }
        let static_dir = p.root.join("static");
        if static_dir.is_dir() {
            let mut all = Vec::new();
            list_files(&static_dir, &mut all)?;
            all.sort();
            for (f, etag) in all.iter().zip(hash_files(&all)) {
                let path = f.strip_prefix(&static_dir).unwrap_or(f);
                let url = format!(
                    "/{}",
                    encode_path(&path.to_string_lossy().replace('\\', "/"))
                );
                files.push((url, f.clone(), etag?));
            }
        }
        for (i, (_, file, etag)) in files.iter().enumerate() {
            let ext = file
                .extension()
                .map(|e| e.to_string_lossy().to_ascii_lowercase())
                .unwrap_or_default();
            self.line(0, &format!(
                "static ASSET_{i}: ::wisp::Asset = ::wisp::Asset {{ body: include_bytes!({}), ext: {}, etag: {} }};",
                lit(&file.to_string_lossy()),
                lit(&ext),
                lit(&format!("\"{etag}\""))
            ));
        }
        self.line(0, "");
        Ok(Assets { css_hash, files })
    }

    /// The `wisp::App` impl, and the TypeScript client of the endpoints.
    fn app(&mut self, p: &Project, web: &Web, assets: &Assets) -> Result<String, String> {
        let css = match &assets.css_hash {
            Some(h) if p.release => Some(h.as_str()),
            Some(_) => Some("dev"),
            None => None,
        };
        // Pages the same for every request, with their response heads.
        let baked = p.baked(css);
        for (i, doc) in baked.iter().enumerate() {
            let Some(doc) = doc else { continue };
            let etag = format!("\"{:016x}\"", fnv1a(doc.as_bytes()));
            let head = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: text/html; charset=utf-8\r\netag: {etag}\r\ncontent-length: {}\r\n",
                doc.len()
            );
            self.line(
                0,
                &format!(
                    "static BAKED_{i}: ::wisp::rt::Baked = ::wisp::rt::Baked::new({}, {}, {}); // {}",
                    lit(&head),
                    lit(doc),
                    lit(&etag),
                    p.model.routes[i].pattern
                ),
            );
        }
        if baked.iter().any(Option::is_some) {
            self.line(0, "");
        }
        self.line(0, "pub struct App;");
        self.line(0, "");
        self.line(
            0,
            "#[allow(unused_imports, unused_variables, unreachable_patterns)]",
        );
        self.line(0, "impl ::wisp::App for App {");
        self.line(
            1,
            &format!(
                "const ROOT: &'static str = {};",
                lit(&p.root.to_string_lossy())
            ),
        );
        let css = css.map_or("None".into(), |v| format!("Some({})", lit(v)));
        self.line(1, &format!("const CSS: Option<&'static str> = {css};"));
        self.routes(p, assets);
        // Template 0 is the shell.
        let shell = [format!(
            "({}, 0x{:016x})",
            lit("src/app.html"),
            shell::SHAPE
        )];
        let tpls: Vec<String> = shell
            .into_iter()
            .chain(
                p.templates
                    .iter()
                    .map(|t| format!("({}, 0x{:016x})", lit(&t.rel), t.t.shape)),
            )
            .collect();
        self.line(
            1,
            &format!(
                "const TEMPLATES: &'static [(&'static str, u64)] = &[{}];",
                tpls.join(", ")
            ),
        );
        self.line(0, "");
        self.router(p);
        let client = self.api(p)?;
        self.init(p);

        self.line(1, "fn shell() -> [&'static str; 3] {");
        if p.release {
            self.line(2, "tpl_shell::S");
        } else {
            self.line(
                2,
                "[0, 1, 2].map(|i| ::wisp::rt::chunk(0, i, tpl_shell::S[i]))",
            );
        }
        self.line(1, "}");
        self.line(0, "");

        // Asked on every GET before routing: a binary search of the paths,
        // sorted, not a comparison with each.
        self.line(
            1,
            "fn asset(path: &str) -> Option<&'static ::wisp::Asset> {",
        );
        if assets.files.is_empty() {
            self.line(2, "let _ = path;");
            self.line(2, "None");
        } else {
            let mut by_url: Vec<(&str, usize)> = (assets.files.iter())
                .enumerate()
                .map(|(i, (url, _, _))| (url.as_str(), i))
                .collect();
            by_url.sort_unstable();
            let table: Vec<String> = by_url
                .iter()
                .map(|(url, i)| format!("({}, &ASSET_{i})", lit(url)))
                .collect();
            self.line(
                2,
                &format!(
                    "static BY_PATH: [(&str, &::wisp::Asset); {}] = [{}];",
                    table.len(),
                    table.join(", ")
                ),
            );
            self.line(
                2,
                "BY_PATH.binary_search_by(|(p, _)| (*p).cmp(path)).ok().map(|i| BY_PATH[i].1)",
            );
        }
        self.line(1, "}");
        self.line(0, "");

        self.line(
            1,
            "fn client_module(path: &str) -> Option<&'static ::wisp::ClientModule> {",
        );
        let modules: Vec<(&Tpl, &Client)> = p
            .templates
            .iter()
            .zip(&web.clients)
            .filter_map(|(t, c)| Some((t, c.as_ref()?)))
            .collect();
        if modules.is_empty() && web.js_files.is_empty() {
            self.line(2, "let _ = path;");
            self.line(2, "None");
        } else {
            self.line(2, "match path {");
            for (t, c) in modules {
                self.line(
                    3,
                    &format!("{} => Some(&{}::__WISP_CLIENT),", lit(&c.path()), t.path()),
                );
            }
            for (i, f) in web.js_files.iter().enumerate() {
                self.line(3, &format!("{} => Some(&__WISP_JS_{i}),", lit(&f.path)));
            }
            self.line(3, "_ => None,");
            self.line(2, "}");
        }
        self.line(1, "}");
        self.line(0, "");

        // Routes for `wisp build --static`.
        self.line(1, "fn export_routes() -> Vec<::wisp::ExportRoute> {");
        self.line(2, "vec![");
        for r in &p.model.routes {
            let page = r.page.as_ref();
            let actions = page.is_some_and(|pg| pg.actions().next().is_some());
            let entries = match page.filter(|pg| pg.entries()) {
                Some(pg) => format!("Some({}::__call::entries)", pg.module),
                None => "None".into(),
            };
            self.line(3, &format!(
                "::wisp::ExportRoute {{ pattern: {}, page: {}, actions: {actions}, server: {}, entries: {entries} }},",
                lit(&r.pattern),
                page.is_some(),
                r.server.is_some()
            ));
        }
        self.line(2, "]");
        self.line(1, "}");
        self.line(0, "");

        self.handle(p, &baked);
        self.handle_now(p);
        self.error(p);
        self.line(0, "}");
        Ok(client)
    }

    /// The router. A path with no parameter in it is matched whole: by its
    /// length, then by a byte that tells the paths of that length apart,
    /// then by all of it; no other arm that matches the same path comes
    /// before it (see `routes::priority`). The rest are tried in turn, most
    /// specific arm first, each walking the path from its start with byte
    /// prefixes, their parameters slices of it. An arm with a `[...rest]`
    /// matches the path's segments with a slice pattern.
    fn router(&mut self, p: &Project) {
        let tree = &p.tree;
        let (whole, parts): (Vec<_>, Vec<_>) = tree
            .arms()
            .into_iter()
            .partition(|(exp, _)| exp.iter().all(|s| matches!(s, Seg::Static(_))));
        self.line(
            1,
            "fn route(path: &str) -> Option<(usize, [&str; ::wisp::rt::MAX_PARAMS])> {",
        );
        if whole.is_empty() && parts.is_empty() {
            self.line(2, "let _ = path;");
            self.line(2, "None");
        }
        if !whole.is_empty() {
            let mut paths: Vec<(String, usize)> = whole
                .iter()
                .map(|(exp, id)| {
                    let mut s: String = exp
                        .iter()
                        .map(|seg| match seg {
                            Seg::Static(x) => format!("/{}", encode_path(x)),
                            _ => unreachable!("partitioned"),
                        })
                        .collect();
                    if s.is_empty() {
                        s.push('/');
                    }
                    (s, *id)
                })
                .collect();
            paths.sort_by(|a, b| a.0.len().cmp(&b.0.len()).then_with(|| a.0.cmp(&b.0)));
            let (open, found) = match parts.is_empty() {
                true => ("", "(@, [\"\"; ::wisp::rt::MAX_PARAMS])"),
                false => ("let whole = ", "@"),
            };
            let arm = |s: &str, id: usize| {
                format!(
                    "(path == {}).then_some({}), // {}",
                    lit(s),
                    found.replace('@', &id.to_string()),
                    tree.routes[id].pattern()
                )
            };
            self.line(2, &format!("{open}match path.len() {{"));
            let mut at = 0;
            while at < paths.len() {
                let len = paths[at].0.len();
                let n = paths[at..]
                    .iter()
                    .take_while(|(s, _)| s.len() == len)
                    .count();
                let bucket = &paths[at..at + n];
                at += n;
                // A byte where each path of the length has its own.
                let tells = (0..len).find(|&i| {
                    let mut b: Vec<u8> = bucket.iter().map(|(s, _)| s.as_bytes()[i]).collect();
                    b.sort_unstable();
                    b.windows(2).all(|w| w[0] != w[1])
                });
                match tells {
                    _ if n == 1 => {
                        let (s, id) = &bucket[0];
                        self.line(3, &format!("{len} => {}", arm(s, *id)));
                    }
                    Some(i) => {
                        self.line(3, &format!("{len} => match path.as_bytes()[{i}] {{"));
                        for (s, id) in bucket {
                            self.line(4, &format!("{} => {}", s.as_bytes()[i], arm(s, *id)));
                        }
                        self.line(4, "_ => None,");
                        self.line(3, "},");
                    }
                    None => {
                        self.line(3, &format!("{len} => match path {{"));
                        for (s, id) in bucket {
                            let found = found.replace('@', &id.to_string());
                            let pattern = tree.routes[*id].pattern();
                            self.line(4, &format!("{} => Some({found}), // {pattern}", lit(s)));
                        }
                        self.line(4, "_ => None,");
                        self.line(3, "},");
                    }
                }
            }
            self.line(3, "_ => None,");
            if parts.is_empty() {
                self.line(2, "}");
            } else {
                self.line(2, "};");
                self.line(2, "if let Some(id) = whole {");
                self.line(3, "return Some((id, [\"\"; ::wisp::rt::MAX_PARAMS]));");
                self.line(2, "}");
            }
        }
        if parts.is_empty() {
            self.line(1, "}");
            self.line(0, "");
            return;
        }
        self.line(2, "const E: &str = \"\";");
        // What follows the path's first `/`; a path without one matches none.
        self.line(2, "let r0 = path.strip_prefix('/')?;");
        for (a, (exp, id)) in parts.into_iter().enumerate() {
            let r = &tree.routes[id];
            let names = r.params();
            let mut values = vec!["E".to_string(); crate::routes::MAX_PARAMS];
            let rest = exp.iter().any(|s| matches!(s, Seg::Rest(_)));
            // A matcher is a guard, so a segment it refuses goes on to the next arm.
            let mut guards = Vec::new();
            let mut guard = |k: usize, m: &Option<String>| {
                let decoded = format!("&::wisp::rt::decode(p{k}.as_bytes(), false)");
                match tree.matchers.iter().find(|(x, _)| Some(x) == m.as_ref()) {
                    Some((m, Some(_))) => {
                        guards.push(format!("param_{m}::__call::matches({decoded})"))
                    }
                    // `int`: digits that fit a u64, so `parse().unwrap()` holds.
                    Some(_) => guards.push(format!(
                        "p{k}.bytes().all(|b| b.is_ascii_digit()) && p{k}.parse::<u64>().is_ok()"
                    )),
                    None => {}
                }
            };
            let (end, pattern) = (
                format!("return Some(({id}, [@]));"),
                format!("// {}", r.pattern()),
            );
            if rest {
                // The slice pattern of the path's segments, as deep as any.
                let mut pat = Vec::new();
                for (k, seg) in exp.iter().enumerate() {
                    match seg {
                        Seg::Static(s) => pat.push(lit(&encode_path(s))),
                        Seg::Param(n, m) | Seg::Optional(n, m) => {
                            pat.push(format!("p{k}"));
                            values[names.iter().position(|x| x == n).unwrap()] = format!("*p{k}");
                            guard(k, m);
                        }
                        Seg::Rest(n) => {
                            pat.push(format!("p{k} @ .."));
                            values[names.iter().position(|x| x == n).unwrap()] =
                                format!("::wisp::rt::rest(path, p{k})");
                        }
                    }
                }
                let guards: String = guards.iter().map(|g| format!(" && {g}")).collect();
                self.line(2, "{");
                self.line(3, "let mut segs = [\"\"; ::wisp::rt::MAX_SEGS];");
                self.line(
                    3,
                    &format!(
                        "if let Some([{}]) = ::wisp::rt::split(path, &mut segs){guards} {{ {} }} {pattern}",
                        pat.join(", "),
                        end.replace('@', &values.join(", "))
                    ),
                );
                self.line(2, "}");
                continue;
            }
            // Each segment in turn from `r`, what follows a `/`: the
            // static ones run together into one prefix.
            let mut steps = Vec::new();
            let mut prefix = String::new();
            let last = exp.len() - 1;
            for (k, seg) in exp.iter().enumerate() {
                match seg {
                    Seg::Static(s) if k < last => prefix.push_str(&(encode_path(s) + "/")),
                    Seg::Static(s) => {
                        prefix.push_str(&encode_path(s));
                        steps.push(format!("if r != {} {{ break 'a{a}; }}", lit(&prefix)));
                        prefix.clear();
                    }
                    Seg::Param(n, m) | Seg::Optional(n, m) => {
                        if !prefix.is_empty() {
                            steps.push(format!(
                                "let Some(r) = r.strip_prefix({}) else {{ break 'a{a}; }};",
                                lit(&prefix)
                            ));
                            prefix.clear();
                        }
                        let after = if k < last { "Some(r)" } else { "None" };
                        steps.push(format!(
                            "let (p{k}, {after}) = ::wisp::rt::seg(r) else {{ break 'a{a}; }};"
                        ));
                        values[names.iter().position(|x| x == n).unwrap()] = format!("p{k}");
                        guard(k, m);
                    }
                    Seg::Rest(_) => unreachable!("arms with one are matched above"),
                }
            }
            for g in &guards {
                steps.push(format!("if !({g}) {{ break 'a{a}; }}"));
            }
            self.line(2, &format!("'a{a}: {{ {pattern}"));
            self.line(3, "let r = r0;");
            for s in steps {
                self.line(3, &s);
            }
            self.line(3, &end.replace('@', &values.join(", ")));
            self.line(2, "}");
        }
        self.line(2, "None");
        self.line(1, "}");
        self.line(0, "");
    }

    /// `App::ROUTES`: every fact the server looks up by route id, a row a
    /// route, so none can drift from the others. `now` is whether a
    /// request is answered without waiting, decided here so the server
    /// pays nothing to know: when `before` in hooks.rs cannot wait and the
    /// model says the route cannot (`Model::route_waits`). Every doubt
    /// counts as waiting: a route wrongly `now` would fail its request
    /// (see `wisp::http::on_driver`).
    fn routes(&mut self, p: &Project, assets: &Assets) {
        let m = &p.model;
        let before = p.before_waits;
        self.line(1, "const ROUTES: &'static [::wisp::rt::RouteFacts] = &[");
        for (r, route) in p.tree.routes.iter().zip(&m.routes) {
            let names: Vec<String> = r.params().iter().map(|p| lit(p)).collect();
            let limit = (route.body_limit.as_ref()).map(|m| format!("{m}::__call::BODY_LIMIT"));
            let uploads = (route.uploads.as_ref()).map(|m| format!("{m}::__call::UPLOADS"));
            let some = |x: Option<String>| x.map_or("None".into(), |x| format!("Some({x})"));
            // An embedded file at one of its paths is served before it.
            let files = (assets.files.iter())
                .any(|(url, ..)| r.expansions().iter().any(|e| may_match(e, url)));
            let sync: Vec<String> = (now_arms(p, route).iter())
                .flat_map(|(_, h)| method_of(h).1.split(" | "))
                .map(|v| format!("::wisp::Method::{v}.bit()"))
                .collect();
            let sync = if sync.is_empty() {
                "0".into()
            } else {
                sync.join(" | ")
            };
            self.line(
                2,
                &format!(
                    "::wisp::rt::RouteFacts {{ params: &[{}], body_limit: {}, uploads: {}, now: {}, sync: {sync}, files: {files}, error: {} }}, // {}",
                    names.join(", "),
                    some(limit),
                    some(uploads),
                    !before && !m.route_waits(route),
                    opt(route.error),
                    route.pattern
                ),
            );
        }
        self.line(1, "];");
        // An unmatched path is answered by the root error page.
        if !before && !m.root_waits() {
            self.line(1, "const NOT_FOUND_NOW: bool = true;");
        }
    }

    /// The OpenAPI document and TypeScript client of the `+server.rs`
    /// endpoints; the client comes back.
    fn api(&mut self, p: &Project) -> Result<String, String> {
        // Types an endpoint names but does not define may be in the app's own
        // modules (`src/models.rs`).
        let shared = &p.shared;
        let types: Vec<(&crate::routes::Route, Vec<Op>, Vec<rust_scan::TypeItem>)> =
            (p.tree.routes.iter().zip(&p.model.routes))
                .filter_map(|(route, r)| {
                    let server = r.server.as_ref()?;
                    let ops = server.handlers.iter().map(|h| h.op.clone()).collect();
                    let types = server.types.iter().chain(shared).cloned().collect();
                    Some((route, ops, types))
                })
                .collect();
        let endpoints: Vec<crate::openapi::Endpoint> = (types.iter())
            .map(|(route, ops, types)| crate::openapi::Endpoint { route, ops, types })
            .collect();
        if endpoints.is_empty() {
            return Ok(String::new());
        }
        let client = crate::openapi::typescript(&endpoints);
        self.line(1, "fn client_ts() -> &'static str {");
        self.line(2, &lit(&client));
        self.line(1, "}");
        let var = |k: &str, or: &str| std::env::var(k).unwrap_or_else(|_| or.into());
        let spec = crate::openapi::spec(
            &var("CARGO_PKG_NAME", "app"),
            &var("CARGO_PKG_VERSION", "0.1.0"),
            &endpoints,
        );
        self.line(1, "fn openapi() -> &'static str {");
        self.line(2, &lit(&spec));
        self.line(1, "}");
        self.line(0, "");
        Ok(client)
    }

    fn init(&mut self, p: &Project) {
        self.line(1, "async fn init() -> ::wisp::Result<()> {");
        if p.has_hook("init") {
            self.line(2, "hooks::__call::init().await?;");
        }
        // Saved tables load now, once `init` has set the store: nothing lists
        // them at run time, and a store that fails stops the server here.
        let mods = p.mods.iter().map(|m| (m, "__mods::"));
        let own = p.hooks.iter().chain(&p.user_mods).map(|m| (m, ""));
        for (m, at) in mods.chain(own) {
            if !m.tables.is_empty() {
                self.line(2, &format!("{at}{}::__call::__ready();", m.name));
            }
        }
        self.line(2, "Ok(())");
        self.line(1, "}");
        self.line(0, "");
    }

    fn handle(&mut self, p: &Project, baked: &[Option<String>]) {
        self.line(1, "async fn handle(route: Option<usize>, cx: &mut ::wisp::Cx, __o: &mut ::wisp::Out) -> ::wisp::Result<()> {");
        self.line(2, "use ::wisp::Method::*;");
        if p.has_hook("before") {
            self.line(2, "// src/hooks.rs");
            self.line(2, &answer("hooks::__call::before"));
        }
        self.line(2, "::wisp::rt::hooked(cx);");
        self.line(
            2,
            "let Some(route) = route else { return Err(::wisp::Error::new(404, \"Not Found\")) };",
        );
        self.line(2, "match (route, cx.method) {");
        for (i, r) in p.model.routes.iter().enumerate() {
            let mut allow: Vec<&str> = Vec::new();
            self.line(3, &format!("// {}", r.pattern));
            // A GET's statements, when `CACHE` keeps it: what this worker
            // has, or else what `serve` answers, kept (apart per `accept`
            // when the answer varies by it).
            let kept = |serve: String| match &r.cache {
                Some(c) => {
                    let (m, public) = (&c.module, c.public);
                    let by = r.by_accept();
                    format!(
                        "if ::wisp::rt::cached::<{by}>(cx, __o, {public}) {{ return Ok(()); }} {serve} \
                         ::wisp::rt::keep::<Self, {by}>(cx, __o, {m}::__call::CACHE, {public});"
                    )
                }
                None => serve,
            };
            if let Some(page) = &r.page {
                let get = match baked[i] {
                    Some(_) => format!(
                        "if ::wisp::rt::baked(cx, __o, &BAKED_{i}) {{ Ok(()) }} else {{ serve_page_{i}(cx, __o).await }}"
                    ),
                    None if r.cache.is_some() => {
                        format!(
                            "{{ {} Ok(()) }}",
                            kept(format!("serve_page_{i}(cx, __o).await?;"))
                        )
                    }
                    None => format!("serve_page_{i}(cx, __o).await"),
                };
                // live.js asks for the page's error page this way when its
                // browser code fails while starting (see `boundary` in
                // live.js): only pages have any.
                self.line(
                    3,
                    &format!("({i}, Get | Head) => {{ ::wisp::rt::browser_ok(cx)?; {get} }},"),
                );
                allow.extend(["GET", "HEAD"]);
                let actions: Vec<&FnItem> = page.actions().collect();
                if !actions.is_empty() {
                    self.line(3, &format!("({i}, Post) => {{"));
                    self.line(4, "::wisp::rt::check_origin(cx)?;");
                    self.line(4, &idempotent("()"));
                    self.line(4, "match cx.action() {");
                    // `invalid(field, ..)` shows the page again, as a 422, with
                    // what is wrong for `cx.problem(field)`.
                    for a in &actions {
                        self.line(
                            5,
                            &format!(
                                "{} => match {}::__call::{}(cx).await {{ \
                                 Ok(Some(r)) => {{ ::wisp::rt::respond(__o, r); return Ok(()); }} \
                                 Ok(None) => {{}} \
                                 Err(e) => ::wisp::rt::input::failed(cx, e)?, }},",
                                lit(&a.name),
                                page.module,
                                a.name
                            ),
                        );
                    }
                    self.line(5, "other => return Err(::wisp::rt::no_action(other)),");
                    self.line(4, "}");
                    self.line(4, &format!("serve_page_{i}(cx, __o).await"));
                    self.line(3, "}");
                    allow.push("POST");
                }
            }
            for (s, h) in (r.server.iter()).flat_map(|s| s.handlers.iter().map(move |h| (s, h))) {
                let (_, variants, allowed) = method_of(h);
                let m = &s.module;
                let mut before = match s.before {
                    true => answer(&format!("{m}::__call::before")) + " ",
                    false => String::new(),
                };
                if h.op.method == "post" {
                    // After `before`'s `if … { … }`, a `;`: not an `else if`.
                    let sep = if before.is_empty() { "" } else { "; " };
                    before = format!("{}{sep}{} ", before.trim_end(), idempotent("()"));
                }
                let mut serve = serve(m, h, &format!("{}(cx).await", h.shim));
                if h.op.method == "get" {
                    serve = kept(serve);
                }
                self.line(
                    3,
                    &format!("({i}, {variants}) => {{ ::wisp::rt::endpoint(cx); {before}{serve} Ok(()) }}"),
                );
                allow.push(allowed);
            }
            // OPTIONS says what the route takes; CORS preflights are answered
            // by `cx.cors` in `before`, which runs first.
            allow.push("OPTIONS");
            let allow = lit(&allow.join(", "));
            self.line(
                3,
                &format!("({i}, Options) => {{ ::wisp::rt::respond(__o, ::wisp::rt::options({allow})); Ok(()) }}"),
            );
            // An endpoint's errors are JSON, whatever its path.
            let api =
                match r.page.is_some() || r.server.as_ref().is_none_or(|s| s.handlers.is_empty()) {
                    true => "",
                    false => "::wisp::rt::endpoint(cx); ",
                };
            self.line(
                3,
                &format!("({i}, _) => {{ {api}Err(::wisp::rt::method_not_allowed({allow})) }}"),
            );
        }
        self.line(3, "_ => Err(::wisp::Error::new(404, \"Not Found\")),");
        self.line(2, "}");
        self.line(1, "}");
        self.line(0, "");
    }

    /// `handle`'s arms that are sync (see `now_arms`), as plain code:
    /// `Ok(false)` for any other, which `handle` answers.
    fn handle_now(&mut self, p: &Project) {
        let arms: Vec<(usize, &model::Route)> = (p.model.routes.iter().enumerate())
            .filter(|(_, r)| !now_arms(p, r).is_empty())
            .collect();
        if arms.is_empty() {
            return;
        }
        self.line(1, "fn handle_now(route: Option<usize>, cx: &mut ::wisp::Cx, __o: &mut ::wisp::Out) -> ::wisp::Result<bool> {");
        self.line(2, "use ::wisp::Method::*;");
        self.line(2, "let Some(route) = route else { return Ok(false) };");
        self.line(2, "match (route, cx.method) {");
        for (i, r) in arms {
            for (s, h) in now_arms(p, r) {
                let before = match h.op.method == "post" {
                    true => idempotent("true") + " ",
                    false => String::new(),
                };
                let mut serve = serve(&s.module, h, &format!("{}_now(cx)", h.shim));
                if let (Some(c), "get") = (&r.cache, h.op.method) {
                    let (m, public, by) = (&c.module, c.public, r.by_accept());
                    serve = format!(
                        "if ::wisp::rt::cached::<{by}>(cx, __o, {public}) {{ return Ok(true); }} {serve} \
                         ::wisp::rt::keep::<Self, {by}>(cx, __o, {m}::__call::CACHE, {public});"
                    );
                }
                self.line(
                    3,
                    &format!(
                        "({i}, {}) => {{ ::wisp::rt::hooked(cx); ::wisp::rt::endpoint(cx); {before}{serve} Ok(true) }}",
                        method_of(h).1
                    ),
                );
            }
        }
        self.line(3, "_ => Ok(false),");
        self.line(2, "}");
        self.line(1, "}");
        self.line(0, "");
    }

    /// The nearest error page per route; unmatched URLs use the root one.
    fn error(&mut self, p: &Project) {
        let m = &p.model;
        self.line(1, "async fn error(route: Option<usize>, cx: &mut ::wisp::Cx, __o: &mut ::wisp::Out, status: u16, message: &str) -> ::wisp::Result<()> {");
        if m.errors.is_empty() {
            self.line(2, "let _ = route;");
            self.line(2, "::wisp::rt::default_error(cx, __o, status, message);");
            self.line(2, "Ok(())");
        } else {
            self.line(
                2,
                &format!(
                    "let page = match route {{ Some(r) => Self::ROUTES[r].error, None => {} }};",
                    opt(m.root_error)
                ),
            );
            self.line(2, "match page {");
            for i in 0..m.errors.len() {
                self.line(
                    3,
                    &format!("Some({i}) => serve_error_{i}(cx, __o, status, message).await,"),
                );
            }
            self.line(
                3,
                "_ => { ::wisp::rt::default_error(cx, __o, status, message); Ok(()) }",
            );
            self.line(2, "}");
        }
        self.line(1, "}");
    }
}

/// The CSS's hash (`None` without CSS), and the files a release build
/// embeds: (URL, file, etag).
struct Assets {
    css_hash: Option<String>,
    files: Vec<(String, PathBuf, String)>,
}

/// A `+server.rs`, which serves its route, its `/[id]`, or both.
struct ServerFile {
    file: PathBuf,
    /// It sets `BODY_LIMIT`.
    limit: bool,
    /// It sets `CACHE` (`false`) or `CACHE_PUBLIC` (`true`).
    cache: Option<bool>,
    /// All it serves: each of its routes takes its own handlers of it.
    server: model::Server,
}

/// The handlers of a `+server.rs` whose route is `segs`, their shims added
/// to `shims`, and whether it has `before`. A `#[derive(Rest)]` type
/// answers each method (on the route and its `/[id]`) the file does not.
fn server_handlers(
    items: &rust_scan::Items,
    segs: &[Seg],
    shims: &mut Vec<String>,
) -> Result<(Vec<Handler>, bool), String> {
    use crate::routes::{HANDLERS, is_member, rest_type};
    let mut handlers: Vec<Handler> = Vec::new();
    let mut before = false;
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
            let op = Op {
                method,
                inputs,
                value,
                fallible: false,
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
const REST_HOOKS: [(&str, bool); 6] = [
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
fn rest_hooks(items: &rust_scan::Items, ty: &str) -> Result<Vec<String>, String> {
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
fn check_before(f: &FnItem) -> Result<(), String> {
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
const METHODS: [(&str, &str, &str); 5] = [
    ("get", "Get | Head", "GET, HEAD"),
    ("post", "Post", "POST"),
    ("put", "Put", "PUT"),
    ("patch", "Patch", "PATCH"),
    ("delete", "Delete", "DELETE"),
];

/// Whether the path `url` (`/a/b.png`, encoded) may be one the route arm
/// `exp` matches: by its segments, a parameter any but an empty one (a
/// matcher's, any), a `[...rest]` anything.
fn may_match(exp: &[&Seg], url: &str) -> bool {
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

/// The statement in a POST's arm of `handle`, once its hooks passed, that
/// answers a request whose `Idempotency-Key` was answered before.
/// The statement that answers a POST its `Idempotency-Key` answered
/// before, returning `Ok(done)`.
fn idempotent(done: &str) -> String {
    format!("if ::wisp::rt::idempotent(cx, __o) {{ return Ok({done}); }}")
}

/// The method `h` answers: its name, its `Method` variants, and its `Allow` names.
fn method_of(h: &Handler) -> &'static (&'static str, &'static str, &'static str) {
    METHODS.iter().find(|(n, _, _)| *n == h.op.method).unwrap()
}

/// The arms of route `r` that `App::handle_now` answers without a future:
/// the `+server.rs` handlers with a sync twin (`Handler::sync`), on a route
/// that never waits, with no `before` hook of the app's or the file's.
fn now_arms<'a>(p: &Project, r: &'a model::Route) -> Vec<(&'a model::Server, &'a Handler)> {
    let now = !p.has_hook("before") && !p.model.route_waits(r);
    (r.server.iter().filter(|s| now && !s.before))
        .flat_map(|s| s.handlers.iter().filter(|h| h.sync).map(move |h| (s, h)))
        .collect()
}

/// The statement in `handle` that runs an `Answer` shim (an action or
/// `before`): a `Response` it hands back is sent instead of the page.
fn answer(shim: &str) -> String {
    format!("if let Some(r) = {shim}(cx).await? {{ ::wisp::rt::respond(__o, r); return Ok(()); }}")
}

/// Sets a route's `BODY_LIMIT` or `CACHE`, which its page (in file
/// `page`) and the `+server.rs` beside it cannot both set.
fn set_once<T>(slot: &mut Option<T>, v: T, name: &str, page: &str) -> Result<(), String> {
    if slot.is_some() {
        return Err(format!(
            "`{name}` is also set in this route's {page}; set it in one place"
        ));
    }
    *slot = Some(v);
    Ok(())
}

/// `src/hooks.rs`, if there is one, checked, as a module with shims for
/// the hooks it has: `init` and `before`, which look the way they are
/// called, and no other public function (a typo would never run). With it,
/// whether `before` can make a request wait (`init` runs before any).
fn hooks(root: &Path) -> Result<(Option<UserMod>, bool), String> {
    // A `mod hooks;` of the app's own would compile the file a second time,
    // with statics of its own.
    let main = root.join("src").join("main.rs");
    if let Ok(src) = crate::read_source(&main) {
        for (n, line) in src.lines().enumerate() {
            let t = line.trim_start().trim_start_matches("pub ").trim_start();
            if t.starts_with("mod hooks;") || t.starts_with("mod hooks ") {
                return Err(format!(
                    "src/main.rs:{}: remove `mod hooks`: Wisp includes src/hooks.rs itself",
                    n + 1
                ));
            }
        }
    }
    let file = root.join("src").join("hooks.rs");
    if !file.exists() {
        return Ok((None, false));
    }
    let src = crate::read_source(&file).map_err(|e| format!("src/hooks.rs: {e}"))?;
    let items = rust_scan::scan(&src).map_err(|e| format!("src/hooks.rs:{e}"))?;
    items
        .check_inner()
        .map_err(|e| format!("src/hooks.rs:{e}"))?;
    let mut shims = Vec::new();
    for f in &items.fns {
        let at = |msg: String| format!("src/hooks.rs:{}: {msg}", f.line);
        if f.action {
            return Err(at(format!(
                "`{}` is marked #[action], but actions belong in a +page.rs",
                f.name
            )));
        }
        match f.name.as_str() {
            "init" => {
                if !f.params.is_empty() {
                    return Err(at("`init` runs once, before the server takes requests, so it has no `cx`: `async fn init()`".into()));
                }
                if f.returns_kind() != Returns::Nothing {
                    return Err(at(format!(
                        "`init` returns `{}`; it returns nothing, or `Result<()>` so it can use `?`",
                        f.returns
                    )));
                }
                shims.push(shim(f, Shim::Init).map_err(|e| format!("src/hooks.rs:{e}"))?);
            }
            "before" => {
                check_before(f).map_err(at)?;
                shims.push(shim(f, Shim::Answer).map_err(|e| format!("src/hooks.rs:{e}"))?);
            }
            name if f.public => {
                return Err(at(format!(
                    "`{name}` is not a hook: src/hooks.rs has `init` and `before`. Make it private if it is a helper."
                )));
            }
            _ => {}
        }
    }
    let waits = items.function("before").is_some_and(|f| f.is_async);
    Ok((
        Some(UserMod::new("hooks".into(), file, None, shims, &items)),
        waits,
    ))
}

/// The app's own modules: each `src/NAME.rs` that `main.rs` and `lib.rs` do
/// not declare themselves (with `mod NAME;`), but for `main.rs`, `lib.rs`
/// and `hooks.rs`. Wisp compiles each as `crate::NAME` with the prelude in
/// scope, and route files and templates reach it as `NAME`.
fn app_mods(root: &Path) -> Result<Vec<UserMod>, String> {
    let src = root.join("src");
    let mut declared = String::new();
    for f in ["main.rs", "lib.rs"] {
        declared.push_str(&crate::read_source(&src.join(f)).unwrap_or_default());
        declared.push('\n');
    }
    let mut out = Vec::new();
    let Ok(dir) = fs::read_dir(&src) else {
        return Ok(out);
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
        out.push(UserMod::new(name.into(), file, None, Vec::new(), &items));
    }
    Ok(out)
}

/// Whether `src` has `mod NAME;` (`pub mod`, with attributes, anywhere).
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

fn opt(x: Option<usize>) -> String {
    x.map_or("None".into(), |i| format!("Some({i})"))
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
fn check_components(
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
        if crate::uses_tailwind(&text) {
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
            .map(|b| format!("{:016x}", fnv1a(&b)))
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

fn list_files(dir: &Path, out: &mut Vec<PathBuf>) -> Result<(), String> {
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
        return cond.to_string();
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

struct Gen {
    out: String,
    release: bool,
}

impl Gen {
    fn line(&mut self, indent: usize, s: &str) {
        for _ in 0..indent {
            self.out.push_str("    ");
        }
        self.out.push_str(s);
        self.out.push('\n');
    }

    /// Opens `pub mod NAME {` with an app file in it: its own `//!` docs
    /// and `#![…]` attributes first (only possible with the file written
    /// into the module), `use {glob};`, the file (included, so errors point
    /// at it, unless it has inner attributes; a `---` block's items line by
    /// line, each marked with its line), then the prelude.
    fn user_mod(&mut self, m: &UserMod, rel: &str, glob: &str) -> Result<(), String> {
        self.line(0, &format!("pub mod {} {{", m.name));
        let src = match &m.inline {
            Some(s) => s.clone(),
            None => crate::read_source(&m.file).map_err(|e| format!("{rel}: {e}"))?,
        };
        let top = rust_scan::inner_end(&src);
        if top > 0 {
            self.out.push_str(&src[..top]);
            self.out.push('\n');
        }
        self.line(1, "#[allow(unused_imports)]");
        self.line(1, &format!("use {glob};"));
        if m.inline.is_some() {
            let first = src[..top].matches('\n').count() + 1;
            self.rust_lines(1, &src[top..], first, rel);
        } else if top > 0 {
            self.out.push_str(&src[top..]);
            self.out.push('\n');
        } else {
            self.line(1, &format!("include!({});", lit(&m.file.to_string_lossy())));
        }
        // After the file: of two imports of the same names rustc counts the
        // first as used, so a file's own `use wisp::prelude::*` is not
        // reported unused, and this one is allowed to be.
        self.line(1, "#[allow(unused_imports)]");
        self.line(1, "use ::wisp::prelude::*;");
        Ok(())
    }

    /// The `__call` module of `m`, inside its module: where the generated
    /// code calls its functions.
    fn call_mod(&mut self, m: &UserMod, twins: &[String]) {
        self.line(1, "#[doc(hidden)]");
        self.line(1, "#[allow(unused_variables, clippy::all)]");
        self.line(1, "pub mod __call {");
        // The file's names and the prelude, for what `#[validate(…)]`
        // rules name: `max_size = 1 * MB`, `max_len = MAX`.
        self.line(2, "#[allow(unused_imports)]");
        self.line(2, "use super::*;");
        for s in m.calls() {
            for line in s.lines() {
                let twin = (line.strip_prefix("pub fn "))
                    .and_then(|l| l.split_once("_now(cx: &mut ::wisp::Cx)"));
                if let Some((name, _)) = twin
                    && !twins.contains(&format!("{}::{name}", m.name))
                {
                    continue;
                }
                self.line(2, line);
            }
        }
        self.line(1, "}");
    }

    /// Rust from a `.wisp` file, whose first line is line `first` of it,
    /// each line marked with where it is (`// file.wisp:7`) so that rustc's
    /// errors are told against the file. Blank lines are left out, and a
    /// line inside a string is written as it is.
    fn rust_lines(&mut self, ind: usize, code: &str, first: usize, rel: &str) {
        let ends = rust_scan::line_ends_in_code(code);
        let mut inside = false;
        for (k, l) in code.split('\n').enumerate() {
            let safe = ends.get(k).copied().unwrap_or(true);
            if !inside && l.trim().is_empty() {
                continue;
            }
            if inside {
                self.out.push_str(l);
            } else {
                self.out.push_str(&"    ".repeat(ind));
                self.out.push_str(l.trim_end());
            }
            if safe {
                let _ = write!(self.out, " // {rel}:{}", first + k);
            }
            self.out.push('\n');
            inside = !safe;
        }
    }

    fn template(&mut self, t: &Tpl, comps: &[Comp], client: Option<&Client>) {
        self.line(0, &format!("// {}", t.rel));
        self.line(0, "#[doc(hidden)]");
        self.line(0, "#[allow(unused_imports, unused_variables, unused_mut, unused_parens, unused_braces, unused_macros, dead_code, clippy::all)]");
        self.line(0, &format!("pub mod {} {{", t.module));
        // How `{expr}` is written: see `wisp::rt::Text`.
        self.line(
            1,
            "use ::wisp::rt::{Always as _, Direct as _, Formatted as _, Maybe as _};",
        );
        // Inside the module of its `+page.rs` (or `+layout.rs`), it sees
        // what the file sees, private items and the prelude too.
        // Otherwise it sees the app's modules (`db::find`).
        if t.user.is_some() {
            self.line(1, "use super::*;");
        } else {
            self.line(1, "use super::__mods::*;");
        }
        if !self.release {
            let chunks: Vec<String> = t.t.chunks.iter().map(|c| lit(c)).collect();
            // Named so that no name in the app's code can collide with them.
            self.line(
                1,
                &format!(
                    "static __WISP_S: [&str; {}] = [{}];",
                    chunks.len(),
                    chunks.join(", ")
                ),
            );
            self.line(1, "#[inline(always)]");
            self.line(1, &format!("fn __wisp_s(i: usize) -> &'static str {{ ::wisp::rt::chunk({}, i, __WISP_S[i]) }}", t.id));
        }
        if let Some(c) = client {
            let (path, url) = (c.path(), format!("{}?v={}", c.path(), c.hash));
            self.line(1, &format!(
                "pub static __WISP_CLIENT: ::wisp::ClientModule = ::wisp::ClientModule {{ id: {}, path: {}, url: {}, etag: {}, source: {}, preload: {} }};",
                lit(&c.id),
                lit(&path),
                lit(&url),
                lit(&format!("\"{}\"", c.hash)),
                lit(&c.source),
                lit(c.extra.as_deref().unwrap_or(""))
            ));
        }
        // A load hands its `Data` over; statements are in the render itself.
        let loaded = t.stmts.is_none() && matches!(t.user, Some((_, true)));
        let data = if loaded {
            ", __d: &super::__call::Loaded"
        } else {
            ""
        };
        let sig = match t.kind {
            // The statements run first, and may await and use `cx` mutably;
            // then `__wrap` renders the layouts around the page.
            Kind::Page if t.stmts.is_some() => "pub async fn render(cx: &mut ::wisp::Cx, __o: &mut ::wisp::Out, \
                 __wrap: impl FnOnce(&mut ::wisp::Out, &::wisp::Cx, &dyn Fn(&mut ::wisp::Out))) -> ::wisp::Result<()>"
                .into(),
            Kind::Page => format!("pub fn render(__o: &mut ::wisp::Out, cx: &::wisp::Cx{data})"),
            Kind::Layout => format!(
                "pub fn render(__o: &mut ::wisp::Out, cx: &::wisp::Cx{data}, children: &dyn Fn(&mut ::wisp::Out))"
            ),
            Kind::Error => {
                "pub fn render(__o: &mut ::wisp::Out, cx: &::wisp::Cx, status: u16, message: &str)"
                    .into()
            }
            Kind::Component => {
                let props: String =
                    t.t.props
                        .iter()
                        .flat_map(|(ds, _)| ds)
                        .map(|d| format!(", {}: {}", d.name, d.ty))
                        .collect();
                format!(
                    "pub fn render(__o: &mut ::wisp::Out{props}, children: &dyn Fn(&mut ::wisp::Out))"
                )
            }
        };
        match &t.t.props {
            Some((_, line)) => self.line(1, &format!("{sig} {{ // {}:{line}", t.rel)),
            None => self.line(1, &format!("{sig} {{")),
        }
        // `data.count` is also `count`: `Copy` fields by value, the rest by
        // reference. A local of the same name shadows it.
        if loaded {
            self.line(2, "let data: &super::Data = &__d.0;");
            for (name, t) in &t.data {
                if !matches!(name.as_str(), "data" | "children") {
                    let by = if ty::is_copy(t) { "" } else { "&" };
                    self.line(2, &format!("let {name} = {by}data.{name};"));
                }
            }
        }
        if let Some((stmts, binds)) = &t.stmts {
            for b in binds {
                self.line(2, b);
            }
            self.rust_lines(2, stmts, 1, &t.rel);
            if t.kind == Kind::Page {
                self.line(2, "let cx: &::wisp::Cx = cx;");
                self.line(2, "__wrap(__o, cx, &|__o: &mut ::wisp::Out| {");
            }
            // Browser code reads the statements' names as `data.name`.
            let names: Vec<&str> = client
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
                });
            if !names.is_empty() {
                let params: Vec<String> = (0..names.len()).map(|k| format!("T{k}")).collect();
                let fields: Vec<String> = names
                    .iter()
                    .zip(&params)
                    .map(|(n, p)| format!("{n}: {p}"))
                    .collect();
                let values: Vec<String> = names.iter().map(|n| format!("{n}: &{n}")).collect();
                self.line(
                    2,
                    &format!(
                        "struct __WispData<{}> {{ {} }}",
                        params.join(", "),
                        fields.join(", ")
                    ),
                );
                self.line(
                    2,
                    &format!("let data = __WispData {{ {} }};", values.join(", ")),
                );
            }
        }
        if let Some(c) = client {
            // Each render is an instance: its id marks its elements, and its
            // record carries the server values its code reads.
            self.line(2, "let __wisp_i = {");
            self.line(
                3,
                "let (__wisp_i, __b) = ::wisp::rt::live(__o, &__WISP_CLIENT);",
            );
            // The runtime closes the record after it.
            self.json(3, "__b", &c.blob, &t.rel);
            self.line(3, "__wisp_i");
            self.line(2, "};");
        }
        let mut cx = Emit {
            rel: &t.rel,
            template: &t.t,
            comps,
            // Components are at the top of the generated file.
            top: if t.user.is_some() {
                "super::super::"
            } else {
                "super::"
            },
            target: "body",
            each_depth: 0,
            client,
            env: Vec::new(),
            props: 0,
            inert: false,
            paint: false,
            has_cx: t.kind != Kind::Component,
            locals: t
                .stmts
                .as_ref()
                .map(|(s, _)| rust_scan::let_names(s))
                .unwrap_or_default(),
        };
        let at = self.out.len();
        self.nodes(&t.t.nodes, 2, &mut cx);
        // Its form's fields read what an action refused: once a render.
        if self.out[at..].contains("__refused") {
            self.out
                .insert_str(at, "        let __refused = ::wisp::rt::refused(cx);\n");
        }
        if client.is_some() {
            self.line(2, "::wisp::rt::live_end(__o);");
        }
        if t.kind == Kind::Page && t.stmts.is_some() {
            self.line(2, "});");
            self.line(2, "Ok(())");
        }
        self.line(1, "}");
        // A component the browser renders: its markup as the browser's copy
        // of it, painted from its props' JSON, for a page to show first.
        // Deep enough, a component rendering itself leaves the rest to the
        // browser.
        if let Some(c) = client.filter(|c| c.paints) {
            self.line(1, "pub fn paint(__o: &mut ::wisp::Out, __p: &[::wisp::rt::Js<'_>], children: &dyn Fn(&mut ::wisp::Out), __wisp_d: u32) {");
            self.line(2, "if __wisp_d > 32 { return; }");
            self.line(2, &format!("__o.body.push_str({});", lit(COPY_START)));
            let env: Vec<(String, Pv)> =
                t.t.props
                    .iter()
                    .flat_map(|(ds, _)| ds)
                    .enumerate()
                    .map(|(k, d)| (d.name.clone(), Pv::Val(format!("__p[{k}]"))))
                    .collect();
            let mut cx = Emit {
                props: env.len(),
                env,
                inert: false,
                paint: true,
                client: Some(c),
                ..cx
            };
            self.nodes(&t.t.nodes, 2, &mut cx);
            self.line(2, &format!("__o.body.push_str({});", lit(COPY_END)));
            self.line(1, "}");
        }
        self.line(0, "}");
        self.line(0, "");
    }

    /// Writes `pieces`, JSON with Rust values in it, to `buf` (a `&mut String`).
    fn json(&mut self, ind: usize, buf: &str, pieces: &[Piece], rel: &str) {
        for p in pieces {
            match p {
                Piece::Text(t) => self.line(ind, &format!("{buf}.push_str({});", lit(t))),
                Piece::Value { expr, line } => self.line(
                    ind,
                    &format!("::wisp::rt::json({buf}, &({expr})); // {rel}:{line}"),
                ),
            }
        }
    }

    /// In a release build, a run of text and holes the build can write
    /// (literals) goes out as one `push_str`. A dev build keeps each text
    /// apart: `wisp dev` swaps them by index.
    fn nodes<'n>(&mut self, nodes: impl IntoIterator<Item = &'n Node>, ind: usize, cx: &mut Emit) {
        let mut run = String::new();
        for n in nodes {
            match fold::fixed(n, cx.template).filter(|_| self.release) {
                Some(s) => run.push_str(&s),
                None => {
                    self.text(&mut run, ind, cx);
                    self.node(n, ind, cx);
                }
            }
        }
        self.text(&mut run, ind, cx);
    }

    /// Writes out the run `nodes` gathered, if any, and empties it.
    fn text(&mut self, run: &mut String, ind: usize, cx: &Emit) {
        if !run.is_empty() {
            self.line(ind, &format!("__o.{}.push_str({});", cx.target, lit(run)));
            run.clear();
        }
    }

    fn code_line(&mut self, ind: usize, s: &str, code: &Code, cx: &Emit) {
        self.line(ind, &format!("{s} // {}:{}", cx.rel, code.line));
    }

    fn node(&mut self, n: &Node, ind: usize, cx: &mut Emit) {
        let buf = format!("__o.{}", cx.target);
        match n {
            // A release build writes text in runs (see `nodes`).
            Node::Text(i) => {
                debug_assert!(!self.release);
                self.line(ind, &format!("{buf}.push_str(__wisp_s({i}));"));
            }
            Node::Expr(code) => {
                self.code_line(
                    ind,
                    &format!("(&::wisp::rt::Text(&({}))).put(&mut {buf});", code.src),
                    code,
                    cx,
                );
            }
            Node::UrlStart { prefix } => self.line(
                ind,
                &format!("let __wisp_url = {buf}.len() - {};", prefix.len()),
            ),
            Node::UrlEnd => self.line(
                ind,
                &format!("::wisp::rt::guard_url(&mut {buf}, __wisp_url);"),
            ),
            Node::Attr { name, code, url } => {
                let push = |s: &str| format!("{buf}.push_str({});", lit(s));
                // `{if c { "a" } else { "b" }}`: both written at build time.
                if let Some((cond, yes, no)) = fold::either(&code.src)
                    && !(*url
                        && (crate::contexts::runs_script(&yes)
                            || crate::contexts::runs_script(&no)))
                {
                    let (yes, no) = (format!(" {name}=\"{yes}\""), format!(" {name}=\"{no}\""));
                    let line = format!("if {cond} {{ {} }} else {{ {} }}", push(&yes), push(&no));
                    self.code_line(ind, &line, code, cx);
                    return;
                }
                self.code_line(
                    ind,
                    &format!(
                        "if let Some(__v) = (&::wisp::rt::Attr(&({}))).get() {{",
                        code.src
                    ),
                    code,
                    cx,
                );
                self.line(ind + 1, &push(&format!(" {name}=\"")));
                if *url {
                    self.line(ind + 1, &format!("let __wisp_url = {buf}.len();"));
                }
                self.line(
                    ind + 1,
                    &format!("(&::wisp::rt::Text(__v)).put(&mut {buf});"),
                );
                if *url {
                    self.line(
                        ind + 1,
                        &format!("::wisp::rt::guard_url(&mut {buf}, __wisp_url);"),
                    );
                }
                self.line(ind + 1, &push("\""));
                self.line(ind, "}");
            }
            Node::Bool { name, code, class } => {
                // A class name first in its value (`class="` or `class='`
                // just written) takes no space before it.
                let push = if *class {
                    format!(
                        r#"if !{buf}.ends_with(['"', '\'']) {{ {buf}.push(' '); }} {buf}.push_str({});"#,
                        lit(name)
                    )
                } else {
                    format!("{buf}.push_str({});", lit(&format!(" {name}")))
                };
                self.code_line(ind, &format!("if ({}) {{ {push} }}", code.src), code, cx);
            }
            Node::Html(code) => self.code_line(
                ind,
                &format!("::wisp::rt::html(&mut {buf}, &({}));", code.src),
                code,
                cx,
            ),
            Node::Selected(code) => self.code_line(
                ind,
                &format!(
                    "if ::wisp::rt::is(&__wisp_sel, {}) {{ {buf}.push_str(\" selected\"); }}",
                    code.src
                ),
                code,
                cx,
            ),
            Node::Const(code) => self.code_line(ind, &format!("let {};", code.src), code, cx),
            Node::Kept {
                name,
                sent,
                own,
                line,
            } => {
                let cond = Code {
                    src: format!("let Some(__k) = {}", kept(name, cx.has_cx)),
                    line: *line,
                };
                self.code_line(
                    ind,
                    &format!("if {} {{", if_condition(&cond.src, &cx.locals)),
                    &cond,
                    cx,
                );
                self.nodes(sent, ind + 1, cx);
                if let Some(o) = own {
                    self.line(ind, "} else {");
                    self.nodes(o, ind + 1, cx);
                }
                self.line(ind, "}");
            }
            Node::Chosen { name, own, line } => {
                let own = match own {
                    Some(own) => format!("(&::wisp::rt::Attr(&({own}))).get()"),
                    None => "None::<&str>".into(),
                };
                let code = Code {
                    src: format!(
                        "__wisp_sel = ::wisp::rt::chosen({own}, {})",
                        kept(name, cx.has_cx)
                    ),
                    line: *line,
                };
                self.code_line(ind, &format!("let {};", code.src), &code, cx);
            }
            Node::Problem { .. } if !cx.has_cx => {}
            Node::Problem { name, line, .. } => {
                let code = Code {
                    src: format!("::wisp::rt::problem(&mut {buf}, __refused, {name:?})"),
                    line: *line,
                };
                self.code_line(ind, &format!("{};", code.src), &code, cx);
            }
            Node::Render if cx.paint => {
                self.line(
                    ind,
                    &format!(
                        "{buf}.push_str({});",
                        lit(&format!("<template {SLOT_ATTR}></template>"))
                    ),
                );
                self.line(ind, "children(__o);");
            }
            Node::Render => self.line(ind, "children(__o);"),
            // A macro rather than a closure: each render gives the
            // parameters their types, and the body still sees the names
            // around the definition.
            Node::Snippet {
                name,
                params,
                body,
                line,
            } => {
                let args: String = (0..params.len()).map(|k| format!(", $a{k}:expr")).collect();
                self.line(
                    ind,
                    &format!(
                        "macro_rules! {} {{ ($__o:expr{args}) => {{{{ // {}:{line}",
                        snippet_macro(name),
                        cx.rel
                    ),
                );
                self.line(ind + 1, "let __o: &mut ::wisp::Out = $__o;");
                for (k, p) in params.iter().enumerate() {
                    self.line(ind + 1, &format!("let {p} = $a{k};"));
                }
                self.nodes(body, ind + 1, cx);
                self.line(ind, "}} }");
            }
            Node::RenderSnippet { name, args, local } => {
                let rest = if args.src.is_empty() {
                    String::new()
                } else {
                    format!(", {}", args.src)
                };
                let call = if *local {
                    format!("{}!(__o{rest});", snippet_macro(name))
                } else {
                    format!("{name}(__o{rest});")
                };
                self.code_line(ind, &call, args, cx);
            }
            Node::If {
                branches,
                otherwise,
            } => {
                for (k, (cond, body)) in branches.iter().enumerate() {
                    let kw = if k == 0 { "if" } else { "} else if" };
                    self.code_line(
                        ind,
                        &format!("{kw} {} {{", if_condition(&cond.src, &cx.locals)),
                        cond,
                        cx,
                    );
                    self.nodes(body, ind + 1, cx);
                }
                if let Some(o) = otherwise {
                    self.line(ind, "} else {");
                    self.nodes(o, ind + 1, cx);
                }
                self.line(ind, "}");
            }
            Node::Each {
                iter,
                pat,
                index,
                body,
                otherwise,
            } => {
                let empty = format!("__empty{}", cx.each_depth);
                cx.each_depth += 1;
                // A field path is iterated by reference. Method-call syntax lets
                // autoderef find the impl whether the field is a Vec, a slice
                // reference or a map.
                let amp = if is_place(iter.src.trim(), &cx.locals) {
                    "&"
                } else {
                    ""
                };
                let src = format!("({amp}({})).into_iter()", iter.src);
                let head = match index {
                    Some(i) => format!("for ({i}, {pat}) in {src}.enumerate() {{"),
                    None => format!("for {pat} in {src} {{"),
                };
                if let Some(o) = otherwise {
                    self.line(ind, "{");
                    self.line(ind + 1, &format!("let mut {empty} = true;"));
                    self.code_line(ind + 1, &head, iter, cx);
                    self.line(ind + 2, &format!("{empty} = false;"));
                    self.nodes(body, ind + 2, cx);
                    self.line(ind + 1, "}");
                    self.line(ind + 1, &format!("if {empty} {{"));
                    self.nodes(o, ind + 2, cx);
                    self.line(ind + 1, "}");
                    self.line(ind, "}");
                } else {
                    self.code_line(ind, &head, iter, cx);
                    self.nodes(body, ind + 1, cx);
                    self.line(ind, "}");
                }
                cx.each_depth -= 1;
            }
            Node::Match { scrutinee, arms } => {
                self.code_line(
                    ind,
                    &format!("match {} {{", borrow_place(&scrutinee.src, &cx.locals)),
                    scrutinee,
                    cx,
                );
                for (pat, body) in arms {
                    self.code_line(ind + 1, &format!("{} => {{", pat.src), pat, cx);
                    self.nodes(body, ind + 2, cx);
                    self.line(ind + 1, "}");
                }
                self.line(ind, "}");
            }
            Node::Head(body) => {
                let prev = cx.target;
                cx.target = "head";
                self.nodes(body, ind, cx);
                cx.target = prev;
            }
            Node::Component {
                name,
                props,
                children,
                line,
            } => self.component(name, props, children, *line, ind, cx),
            Node::Client(branches) => self.client_block(branches, ind, cx),
            // `<wisp:element this="…">`'s name: the tag, when the server
            // knows it.
            Node::Tag { group } => {
                let g = &cx.template.groups[*group];
                let js = g
                    .directives
                    .iter()
                    .find(|d| d.kind == Dir::Tag)
                    .and_then(|d| d.value.as_ref())
                    .map_or("", |v| v.src.as_str());
                match paint_value(cx, *group, js).and_then(|v| v.val()) {
                    Some(v) => self.line(
                        ind,
                        &format!(
                            "::wisp::rt::tag_name(&mut {buf}, {v}); // {}:{}",
                            cx.rel, g.line
                        ),
                    ),
                    None => self.line(ind, &format!("{buf}.push_str(\"wisp-element\");")),
                }
            }
            // `{:x}`: the value, when the server knows it.
            Node::Hole { group } => {
                let g = &cx.template.groups[*group];
                let js = g.directives[0]
                    .value
                    .as_ref()
                    .map_or("", |v| v.src.as_str());
                if let Some(put) = paint_value(cx, *group, js).and_then(|v| v.text(&buf)) {
                    self.line(ind, &format!("{put} // {}:{}", cx.rel, g.line));
                }
            }
            Node::Live { group } => self.live(*group, ind, cx),
        }
    }

    /// A component the server renders: its `render`, given its props in the
    /// order it declares them, then its children.
    fn component(
        &mut self,
        name: &str,
        props: &[template::Prop],
        children: &Option<Vec<Node>>,
        line: u32,
        ind: usize,
        cx: &mut Emit,
    ) {
        let c = cx
            .comps
            .iter()
            .find(|c| c.name == *name)
            .expect("check_components found it");
        // Props in the order the component declares them. A reference
        // type takes a borrow of the expression, so `title={post.title}`
        // passes a `&String` where the prop is a `&str`.
        let mut args = String::new();
        // A browser value (a prop only `$props()` names) is any
        // `Json`, text and flags too.
        let any = |v: &PropValue| match v {
            PropValue::Text(text) => Some(format!("&{}", lit(text))),
            PropValue::Flag => Some("&true".into()),
            _ => None,
        };
        for d in &c.props {
            let by_ref = d.ty.starts_with('&');
            let given = props.iter().find(|p| p.name == d.name).map(|p| &p.value);
            if d.name == REST && c.rest {
                let rest: Vec<String> = props
                    .iter()
                    .filter(|p| {
                        !p.name.starts_with("client:") && !c.props.iter().any(|x| x.name == p.name)
                    })
                    .map(|p| {
                        let v = match &p.value {
                            PropValue::Expr(code) => format!("&({})", code.src),
                            v => any(v).unwrap_or_else(|| "&()".into()),
                        };
                        format!("({}, {v} as {DYN})", lit(&p.name))
                    })
                    .collect();
                args.push_str(&format!(", &[{}]", rest.join(", ")));
                continue;
            }
            if d.ty == DYN
                && let Some(v) = given.and_then(any)
            {
                args.push_str(", ");
                args.push_str(&v);
                continue;
            }
            let arg = match given {
                Some(PropValue::Expr(code)) if by_ref => format!("&({})", code.src),
                Some(PropValue::Expr(code)) => format!("({})", code.src),
                Some(PropValue::Text(text)) if by_ref || d.ty.starts_with("impl") => lit(text),
                Some(PropValue::Text(text)) => {
                    format!("::core::convert::Into::into({})", lit(text))
                }
                Some(PropValue::Flag) => "true".into(),
                // A closure over the macro, typed by the prop's `&dyn Fn`.
                Some(PropValue::Snippet { name, arity }) => {
                    let a: String = (0..*arity).map(|k| format!(", __a{k}")).collect();
                    format!(
                        "&|__o: &mut ::wisp::Out{a}| {}!(__o{a})",
                        snippet_macro(name)
                    )
                }
                Some(PropValue::Live(_) | PropValue::Bind(_) | PropValue::On(_)) => {
                    unreachable!("the parser makes such a component a client one")
                }
                None => format!(
                    "({})",
                    d.default
                        .as_deref()
                        .expect("check_components found every required prop")
                ),
            };
            args.push_str(", ");
            args.push_str(&arg);
        }
        let call = format!("{}{}::render(__o{args}", cx.top, c.module);
        // `client:visible` and the like: an island, which starts late.
        let how = props.iter().find_map(|p| {
            let how = template::how(p.name.strip_prefix("client:")?)?;
            match &p.value {
                _ if how.is_empty() => None,
                PropValue::Text(q) if how == ISLAND_MEDIA => Some(template::island(how, q)),
                _ if how == ISLAND_MEDIA => None,
                _ => Some(how.to_string()),
            }
        });
        if let Some(how) = how {
            self.line(ind, &format!("::wisp::rt::live_how(__o, {});", lit(&how)));
        }
        // Snippets among the children are props: defined before the
        // call, in a block of their own.
        let is_snippet = |n: &&Node| matches!(n, Node::Snippet { .. });
        let defs: Vec<&Node> = children.iter().flatten().filter(is_snippet).collect();
        let ind = if defs.is_empty() {
            ind
        } else {
            self.line(ind, "{");
            for d in &defs {
                self.node(d, ind + 1, cx);
            }
            ind + 1
        };
        match children {
            Some(body) => {
                self.line(
                    ind,
                    &format!("{call}, &|__o: &mut ::wisp::Out| {{ // {}:{line}", cx.rel),
                );
                self.nodes(body.iter().filter(|n| !is_snippet(n)), ind + 1, cx);
                self.line(ind, "});");
            }
            None => self.line(
                ind,
                &format!("{call}, &|_: &mut ::wisp::Out| {{}}); // {}:{line}", cx.rel),
            ),
        }
        if !defs.is_empty() {
            self.line(ind - 1, "}");
        }
    }

    /// A directive element's marks: its group, the loop values its
    /// directives read, and the attributes whose values the server knows.
    fn live(&mut self, group: usize, ind: usize, cx: &Emit) {
        let buf = format!("__o.{}", cx.target);
        let c = cx.client.expect("a template with directives has a module");
        let push = |s: String| format!("{buf}.push_str({});", lit(&s));
        if cx.template.groups[group].nested || cx.paint {
            self.line(ind, &push(format!(" {GROUP_ATTR}=\"{group}\"")));
        } else {
            self.line(ind, &push(format!(" {GROUP_ATTR}=\"")));
            self.line(
                ind,
                &format!("(&::wisp::rt::Text(&__wisp_i)).put(&mut {buf});"),
            );
            self.line(ind, &push(format!(".{group}\"")));
        }
        // The loop values its directives read, as JSON.
        let locals = &c.locals[group];
        if !locals.is_empty() {
            self.line(ind, &push(format!(" {LOOP_ATTR}=\"")));
            self.line(ind, "{");
            self.line(ind + 1, "let __j = &mut ::std::string::String::new();");
            self.json(ind + 1, "__j", locals, cx.rel);
            self.line(ind + 1, &format!("::wisp::rt::escape(&mut {buf}, __j);"));
            self.line(ind, "}");
            self.line(ind, &push("\"".into()));
        }
        // `name={:x}` attributes whose value the server knows, and boolean
        // ones such as `:hidden="!open"` (with `let open = false`): the
        // browser would set them first thing, so the page starts so.
        let g = &cx.template.groups[group];
        for d in g.directives.iter().filter(|d| d.kind == Dir::Spread) {
            let js = d.value.as_ref().map_or("", |v| v.src.as_str());
            if let Some(v) = paint_value(cx, group, js).and_then(|v| v.val()) {
                self.line(
                    ind,
                    &format!(
                        "::wisp::rt::js_attrs(&mut {buf}, {:?}, {v}); // {}:{}",
                        d.name, cx.rel, d.line
                    ),
                );
            }
        }
        for d in g.directives.iter().filter(|d| {
            d.kind == Dir::Attr
                && (d.mods == ["{}"] || template::BOOLEAN_ATTRS.contains(&d.name.as_str()))
        }) {
            let js = d.value.as_ref().map_or("", |v| v.src.as_str());
            if let Some(v) = paint_value(cx, group, js).and_then(|v| v.val()) {
                self.line(
                    ind,
                    &format!(
                        "::wisp::rt::js_attr(&mut {buf}, {}, {v}); // {}:{}",
                        lit(&d.name),
                        cx.rel,
                        d.line
                    ),
                );
            }
        }
    }

    /// A client block or component: each branch's `<template>`, whose
    /// content the browser copies (so nothing in it is painted), and after
    /// it, when the server knows the values, the copies the browser would
    /// make, each between `<!--[-->` and `<!--]-->`, which it takes over.
    fn client_block(&mut self, branches: &[(usize, Vec<Node>)], ind: usize, cx: &mut Emit) {
        let (target, tpl) = (cx.target, cx.template);
        let push = |s: &str| format!("__o.{target}.push_str({});", lit(s));
        let (open, close, start, end) = (
            push("<template"),
            push(">"),
            push(COPY_START),
            push(COPY_END),
        );
        // Painted copies of an `{:#each}`, which its `{:else}` needs none of.
        let mut count: Option<String> = None;
        for (k, (group, body)) in branches.iter().enumerate() {
            self.line(ind, &open);
            self.node(&Node::Live { group: *group }, ind, cx);
            self.line(ind, &close);
            let inert = std::mem::replace(&mut cx.inert, true);
            self.nodes(body, ind, cx);
            cx.inert = inert;
            self.line(ind, &push("</template>"));
            let g = &tpl.groups[*group];
            let d = &g.directives[0];
            let js = d.value.as_ref().map_or("", |v| v.src.as_str());
            let at = format!("// {}:{}", cx.rel, g.line);
            match d.kind {
                Dir::Each => {
                    let n = cx.each_depth;
                    let (item, index) = (format!("__wisp_e{n}"), format!("__wisp_k{n}"));
                    let head = match paint_value(cx, *group, js) {
                        Some(Pv::Val(list)) => {
                            format!("for ({index}, {item}) in {list}.items().enumerate() {{ {at}")
                        }
                        // `[x]`, as `{:@render}` passes an argument: one item, `x`.
                        _ => match js
                            .strip_prefix('[')
                            .and_then(|s| s.strip_suffix(']'))
                            .and_then(|x| paint_value(cx, *group, x)?.val())
                        {
                            Some(x) => format!("{{ let ({index}, {item}) = (0usize, {x}); {at}"),
                            None => continue,
                        },
                    };
                    if branches.len() > 1 {
                        count = Some(format!("__wisp_n{n}"));
                        self.line(ind, &format!("let mut __wisp_n{n} = 0usize;"));
                    }
                    self.line(ind, &head);
                    if count.is_some() {
                        self.line(ind + 1, &format!("__wisp_n{n} += 1;"));
                    }
                    let outer = cx.env.len();
                    cx.env.push((d.name.clone(), Pv::Val(item)));
                    if let Some(i) = d.mods.first() {
                        cx.env.push((i.clone(), Pv::Num(format!("Some({index})"))));
                    }
                    cx.each_depth += 1;
                    self.line(ind + 1, &start);
                    self.nodes(body, ind + 1, cx);
                    self.line(ind + 1, &end);
                    cx.each_depth -= 1;
                    cx.env.truncate(outer);
                    self.line(ind, "}");
                }
                Dir::If if k > 0 && count.is_some() => {
                    let n = count.as_deref().unwrap_or_default();
                    self.line(ind, &format!("if {n} == 0 {{ {at}"));
                    self.line(ind + 1, &start);
                    self.nodes(body, ind + 1, cx);
                    self.line(ind + 1, &end);
                    self.line(ind, "}");
                }
                Dir::If => {
                    let Some(v) = paint_value(cx, *group, js) else {
                        continue;
                    };
                    self.line(ind, &format!("if {} {{ {at}", v.test()));
                    self.line(ind + 1, &start);
                    self.nodes(body, ind + 1, cx);
                    self.line(ind + 1, &end);
                    self.line(ind, "}");
                }
                Dir::Comp => self.paint_comp(*group, body, ind, cx),
                // One copy, always: an await as pending, a try as not failed.
                Dir::Key | Dir::Await | Dir::Try => {
                    let outer = cx.env.len();
                    let state = match d.kind {
                        Dir::Await => Some(("__aw", "{\"k\":0}")),
                        Dir::Try => Some(("__tr", "{}")),
                        _ => None,
                    };
                    if let Some((name, json)) = state {
                        cx.env.push((
                            name.into(),
                            Pv::Val(format!("::wisp::rt::Js({})", lit(json))),
                        ));
                    }
                    self.line(ind, &format!("{{ {at}"));
                    self.line(ind + 1, &start);
                    self.nodes(body, ind + 1, cx);
                    self.line(ind + 1, &end);
                    self.line(ind, "}");
                    cx.env.truncate(outer);
                }
                _ => {}
            }
        }
    }

    /// A component the browser renders, painted by its `paint` when every
    /// prop is known. What it is given as children is painted here.
    fn paint_comp(&mut self, group: usize, body: &[Node], ind: usize, cx: &mut Emit) {
        let (target, tpl, comps) = (cx.target, cx.template, cx.comps);
        let g = &tpl.groups[group];
        let d = &g.directives[0];
        let Some(comp) = comps.iter().find(|c| c.name == d.name) else {
            return;
        };
        // What a spread gives is the browser's to work out.
        if d.props.iter().any(|p| p.name == "...") {
            return;
        }
        let mut args = Vec::new();
        for p in &comp.props {
            let arg = match d.props.iter().find(|x| x.name == p.name).map(|x| &x.value) {
                Some(PropValue::Text(s)) => format!("::wisp::rt::Js({})", lit(&js_str(s))),
                Some(PropValue::Flag) => "::wisp::rt::Js(\"true\")".into(),
                Some(PropValue::Live(c) | PropValue::Bind(c)) => {
                    match paint_value(cx, group, &c.src).and_then(|v| v.val()) {
                        Some(v) => v,
                        None => return,
                    }
                }
                _ => "::wisp::rt::Js(\"null\")".into(),
            };
            args.push(arg);
        }
        let depth = if cx.paint { "__wisp_d + 1" } else { "0" };
        self.line(
            ind,
            &format!(
                "{}{}::paint(__o, &[{}], &|__o: &mut ::wisp::Out| {{ // {}:{}",
                cx.top,
                comp.module,
                args.join(", "),
                cx.rel,
                g.line
            ),
        );
        let push = |s: &str| format!("__o.{target}.push_str({});", lit(s));
        self.line(ind + 1, &push(COPY_START));
        self.nodes(body, ind + 1, cx);
        self.line(ind + 1, &push(COPY_END));
        self.line(ind, &format!("}}, {depth});"));
    }
}

// ---- first paint ------------------------------------------------------------
//
// The server paints client blocks, components and `{:…}` whose values it
// knows, so the page shows them before (and without) JavaScript, and the
// browser takes those nodes over. It knows server values (`data`, props,
// Rust loop names), literals, script variables first set to those, and the
// item and index of a client `each` it is painting; `!`, `&&`, `||` and
// `.length` of those. Anything else (a call, a sum, a comparison) is left
// to the browser.

/// A browser value the server works out, as the Rust expression that does:
/// a `wisp::rt::Js`, a length or index (`Option<usize>`), a `bool`, or a
/// `&&`/`||` (a `bool` that only decides: its JavaScript value is an operand).
#[derive(Clone, Debug)]
enum Pv {
    Val(String),
    Num(String),
    Bool(String),
    Test(String),
}

impl Pv {
    /// As `if (x)` tests it.
    fn test(&self) -> String {
        match self {
            Pv::Val(e) => format!("{e}.truthy()"),
            Pv::Num(e) => format!("{e}.is_some_and(|n| n > 0)"),
            Pv::Bool(e) | Pv::Test(e) => e.clone(),
        }
    }

    /// As a `Js` (a component's prop).
    fn val(&self) -> Option<String> {
        match self {
            Pv::Val(e) => Some(e.clone()),
            Pv::Num(e) => Some(format!("::wisp::rt::Js(&::wisp::rt::js_of(&{e}))")),
            Pv::Bool(e) => Some(format!(
                "::wisp::rt::Js(if {e} {{ \"true\" }} else {{ \"false\" }})"
            )),
            Pv::Test(_) => None,
        }
    }

    /// The statement that writes it as `{:x}` shows it.
    fn text(&self, buf: &str) -> Option<String> {
        match self {
            Pv::Val(e) => Some(format!("{e}.text(&mut {buf});")),
            Pv::Num(e) | Pv::Bool(e) => Some(format!("::wisp::rt::js_text(&mut {buf}, &{e});")),
            Pv::Test(_) => None,
        }
    }

    /// `x.a.b`, or `x.length`.
    fn member(self, rest: &[String]) -> Option<Pv> {
        let mut v = self;
        for (k, seg) in rest.iter().enumerate() {
            v = match v {
                Pv::Val(e) if seg == "length" && k + 1 == rest.len() => {
                    Pv::Num(format!("{e}.length()"))
                }
                Pv::Val(e) => Pv::Val(format!("{e}.get({})", lit(seg))),
                _ => return None,
            };
        }
        Some(v)
    }
}

/// The browser value `js`, read by group `group`, if the server knows it.
fn paint_value(cx: &Emit, group: usize, js: &str) -> Option<Pv> {
    let c = cx.client?;
    if cx.inert {
        return None;
    }
    paint_expr(js, &mut |path| resolve(cx, c, Some(group), path, 0))
}

/// A variable's path as the first paint knows it, read in `group`, or in
/// the script (`None`), which sees only server values and the script's
/// own variables. `depth` stops variables set from each other in a circle.
fn resolve(cx: &Emit, c: &Client, group: Option<usize>, path: &[String], depth: u32) -> Option<Pv> {
    let name = &path[0];
    let env = &cx.env[..if group.is_some() {
        cx.env.len()
    } else {
        cx.props
    }];
    if let Some((_, v)) = env.iter().rev().find(|(n, _)| n == name) {
        return v.clone().member(&path[1..]);
    }
    if c.declared.contains(name) {
        let (_, init) = c
            .lets
            .iter()
            .find(|(n, _)| n == name)
            .filter(|_| depth < 8)?;
        return paint_expr(init, &mut |p| resolve(cx, c, None, p, depth + 1))?.member(&path[1..]);
    }
    // A client local the server is not painting.
    if group.is_some_and(|g| cx.template.groups[g].locals.contains(name)) {
        return None;
    }
    let rust = group.is_some_and(|g| c.scopes[g].contains(name));
    if !rust && !(c.server.contains(name) && !cx.paint) {
        return None;
    }
    let place = if c.whole && !rust {
        path[..1].to_vec()
    } else {
        server_path(path)
    };
    Pv::Val(format!(
        "::wisp::rt::Js(&::wisp::rt::js_of(&({})))",
        rust_place(&place)
    ))
    .member(&path[place.len()..])
}

/// `js` worked out by the server, if it can be: see `Pv`. `root` resolves a
/// variable's path (`data.user.name` whole).
fn paint_expr(js: &str, root: &mut dyn FnMut(&[String]) -> Option<Pv>) -> Option<Pv> {
    let t = js::tokens(js);
    let mut k = 0;
    let v = paint_or(js, &t, &mut k, root)?;
    (k == t.len()).then_some(v)
}

type Root<'a> = dyn FnMut(&[String]) -> Option<Pv> + 'a;

fn paint_or(js: &str, t: &[js::Token], k: &mut usize, root: &mut Root) -> Option<Pv> {
    let mut v = paint_and(js, t, k, root)?;
    while t.get(*k).is_some_and(|n| n.text(js) == "||") {
        *k += 1;
        let w = paint_and(js, t, k, root)?;
        v = Pv::Test(format!("({} || {})", v.test(), w.test()));
    }
    Some(v)
}

fn paint_and(js: &str, t: &[js::Token], k: &mut usize, root: &mut Root) -> Option<Pv> {
    let mut v = paint_not(js, t, k, root)?;
    while t.get(*k).is_some_and(|n| n.text(js) == "&&") {
        *k += 1;
        let w = paint_not(js, t, k, root)?;
        v = Pv::Test(format!("({} && {})", v.test(), w.test()));
    }
    Some(v)
}

fn paint_not(js: &str, t: &[js::Token], k: &mut usize, root: &mut Root) -> Option<Pv> {
    let n = *t.get(*k)?;
    let text = n.text(js);
    match (n.kind, text) {
        (JsKind::Punct, "!") => {
            *k += 1;
            let v = paint_not(js, t, k, root)?;
            Some(Pv::Bool(format!("!({})", v.test())))
        }
        (JsKind::Punct, "(") => {
            *k += 1;
            let v = paint_or(js, t, k, root)?;
            (t.get(*k)?.text(js) == ")").then(|| *k += 1)?;
            Some(v)
        }
        (JsKind::Punct, "[" | "{") | (JsKind::Number | JsKind::String, _) => {
            let end = if n.kind == JsKind::Punct {
                (*k + 1..t.len()).find(|&j| t[j].depth <= n.depth)?
            } else {
                *k
            };
            if let Some(json) = literal_json(js, &t[*k..=end]) {
                *k = end + 1;
                return Some(Pv::Val(format!("::wisp::rt::Js({})", lit(&json))));
            }
            paint_composite(js, t, k, end, root)
        }
        (JsKind::Ident, "true" | "false" | "null" | "undefined") => {
            *k += 1;
            let json = if text == "undefined" { "null" } else { text };
            Some(Pv::Val(format!("::wisp::rt::Js({})", lit(json))))
        }
        (JsKind::Ident, _) if !js::is_reserved(text) && !text.starts_with('#') => {
            let mut path = vec![text.to_string()];
            *k += 1;
            while t
                .get(*k)
                .is_some_and(|n| n.kind == JsKind::Punct && n.text(js) == ".")
                && t.get(*k + 1)
                    .is_some_and(|n| n.kind == JsKind::Ident && !n.text(js).starts_with('#'))
            {
                path.push(t[*k + 1].text(js).to_string());
                *k += 2;
            }
            root(&path)
        }
        _ => None,
    }
}

/// A JavaScript literal as JSON: numbers, strings, `true`, `false`, `null`,
/// and arrays and objects of them.
/// An array or object literal (`[a, 'b']`, `{ color: c, on }`) from `t[*k]`
/// to its end, whose values the first paint knows: its JSON, built when
/// the page renders.
fn paint_composite(
    js: &str,
    t: &[js::Token],
    k: &mut usize,
    end: usize,
    root: &mut Root,
) -> Option<Pv> {
    let array = t[*k].text(js) == "[";
    let (mut fmt, mut args) = (String::from(if array { "[" } else { "{{" }), Vec::new());
    let mut j = *k + 1;
    while j < end {
        if !args.is_empty() {
            fmt.push(',');
        }
        if !array {
            // `key: value`, `'key': value` or `name` for `name: name`.
            let n = t[j];
            let key = match n.kind {
                JsKind::Ident => n.text(js).to_string(),
                JsKind::String => js_string(n.text(js))?,
                _ => return None,
            };
            let _ = write!(
                fmt,
                "{}:",
                js_str(&key).replace('{', "{{").replace('}', "}}")
            );
            if t.get(j + 1).is_some_and(|x| x.text(js) == ":") {
                j += 2;
            } else if n.kind != JsKind::Ident {
                return None;
            }
        }
        let v = paint_or(js, t, &mut j, root)?.val()?;
        fmt.push_str("{}");
        args.push(format!("({v}).0"));
        match t.get(j).map(|x| x.text(js)) {
            Some(",") if j < end => j += 1,
            _ if j == end => {}
            _ => return None,
        }
    }
    fmt.push_str(if array { "]" } else { "}}" });
    *k = end + 1;
    Some(Pv::Val(format!(
        "::wisp::rt::Js(&format!({}{}))",
        lit(&fmt),
        args.iter().map(|a| format!(", {a}")).collect::<String>()
    )))
}

fn literal_json(js: &str, t: &[js::Token]) -> Option<String> {
    let mut out = String::new();
    for (k, n) in t.iter().enumerate() {
        let text = n.text(js);
        match n.kind {
            JsKind::Punct => match text {
                "[" | "{" | ":" | "," => out.push_str(text),
                "]" | "}" => {
                    if out.ends_with(',') {
                        out.pop(); // a trailing comma
                    }
                    out.push_str(text);
                }
                "-" if t.get(k + 1).is_some_and(|m| m.kind == JsKind::Number) => out.push('-'),
                _ => return None,
            },
            // As JavaScript shows it: `1.0` is `1`.
            JsKind::Number => {
                let x = text.parse::<f64>().ok().filter(|x| x.is_finite())?;
                let _ = write!(out, "{x}");
            }
            JsKind::String => out.push_str(&js_str(&js_string(text)?)),
            JsKind::Ident if n.key => out.push_str(&js_str(text)),
            JsKind::Ident if matches!(text, "true" | "false" | "null") => out.push_str(text),
            _ => return None,
        }
    }
    Some(out)
}

/// The text of a JavaScript string literal (`'a\'b'`), for the simple
/// escapes; `None` for others.
fn js_string(lit: &str) -> Option<String> {
    let q = lit.chars().next()?;
    let inner = lit.strip_prefix(q)?.strip_suffix(q)?;
    let mut s = String::with_capacity(inner.len());
    let mut chars = inner.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            s.push(c);
            continue;
        }
        s.push(match chars.next()? {
            'n' => '\n',
            't' => '\t',
            'r' => '\r',
            c @ ('\\' | '\'' | '"') => c,
            _ => return None,
        });
    }
    Some(s)
}

struct Emit<'a> {
    rel: &'a str,
    template: &'a Template,
    comps: &'a [Comp],
    /// The path from the template's module to the top of the generated
    /// file, where the components are.
    top: &'static str,
    target: &'static str,
    each_depth: usize,
    client: Option<&'a Client>,
    /// What the first paint knows by name: a painted component's props
    /// (the first `props` of them), then the item and index of each client
    /// `each` being painted.
    env: Vec<(String, Pv)>,
    props: usize,
    /// In a `<template>`'s content, which the browser copies: no first paint.
    inert: bool,
    /// A component's `paint`: its groups are marked as in the browser's copy.
    paint: bool,
    /// A page, layout or error page: what renders has the request, `cx`,
    /// so an action's form shows what it refused (`Node::Kept`). A
    /// component has none.
    has_cx: bool,
    /// The names a `---` block's statements bind, borrowed where they are
    /// iterated or matched on.
    locals: Vec<String>,
}

// ---- browser code -----------------------------------------------------------
//
// A template with a client script or directives compiles to an ES module
// that the page loads (`/_app/c/ID.js`). Its script runs once per instance
// (per render of the template), inside a function whose first parameter
// holds the server values the file's browser code reads, and it returns a
// list of bindings per element (per `Group`). The runtime (`live.js`)
// matches elements to groups by their `data-w`.
//
// Names in browser code are JavaScript's, except the server's: `data` in a
// page or layout that loads, a component's props, and on an element, the
// Rust loop, `if let`, `{:case}` and `{@const}` names around it. Only what
// the code reads is sent: `data.user.name` sends that one field.

/// JSON as a render function writes it: fixed text, and Rust values.
#[derive(Clone, Debug, PartialEq)]
enum Piece {
    Text(String),
    /// A place such as `data.user.name`, written with `wisp::Json`; `line`
    /// is where the browser code reads it, for rustc's errors.
    Value {
        expr: String,
        line: u32,
    },
}

/// A template's browser module, and what each render of it sends.
struct Client {
    /// `t3`, from the template's id.
    id: String,
    /// The URL of `extra.js`, when it imports it.
    extra: Option<String>,
    source: String,
    /// Of `source`, for the module's URL.
    hash: String,
    /// The instance's server values: a JSON object.
    blob: Vec<Piece>,
    /// Per group: the loop values its directives read (a JSON object), or
    /// nothing.
    locals: Vec<Vec<Piece>>,
    /// The components it renders in the browser, whose modules it imports.
    uses: Vec<usize>,
    /// A component some page renders in the browser: it gets a `paint` fn.
    paints: bool,
    // For the first paint (see `resolve`): the server values it knows, the
    // script's top-level names and what each is first set to, and per
    // group the Rust names around it.
    server: Vec<String>,
    /// Server values are props, read whole (their types need not have the
    /// fields the browser code reads), not `data`, read a field at a time.
    whole: bool,
    declared: Vec<String>,
    lets: Vec<(String, String)>,
    scopes: Vec<Vec<String>>,
}

impl Client {
    fn path(&self) -> String {
        format!("{MODULES}{}.js", self.id)
    }
}

/// A JavaScript file served as it is, but for its imports: `src/lib/**.js`
/// and each `+page.js`.
struct JsFile {
    path: String,
    hash: String,
    source: String,
}

/// The helpers every module's function takes. Most are scoped to the
/// instance; the rest are live.js's exports, handed over so a script needs
/// no import for them.
const HELPERS: &str = "tick, untrack, setTimeout, setInterval, requestAnimationFrame, addEventListener, listen, onMount, onDestroy, effect, watch, \
                       derived, store, persisted, emit, setContext, getContext, goto, invalidate, matches, page, navigating, enhance, \
                       context, portal, __wisp_s, __wisp_r, __wisp_d, __wisp_e, __wisp_ep, __wisp_snap, __wisp_props, __wisp_eq";

/// What `client` needs to know beyond the template.
struct ClientCx<'a> {
    comps: &'a [Comp],
    templates: &'a [Tpl],
    /// Some page renders this component in the browser.
    as_client: bool,
    lib_hash: &'a str,
    /// The page's `+page.js`, served at this URL.
    load: Option<String>,
    /// A release build: `$inspect` goes.
    release: bool,
    /// The URL of the runtime's less used half (`extra.js`).
    extra: &'a str,
}

/// The runtime's less used half: served when a module uses it.
use wisp_shared::EXTRA_JS;

/// Whether a directive needs `extra.js` (so does a module whose code makes
/// a Map or a Set, or uses `enhance`, `$state.snapshot` or `persisted`).
fn is_extra(d: &Directive) -> bool {
    matches!(
        d.kind,
        Dir::Transition
            | Dir::Animate
            | Dir::Await
            | Dir::Try
            | Dir::Tag
            | Dir::Spread
            | Dir::Wait
            | Dir::Comp
    ) || (d.kind == Dir::Bind && !matches!(d.name.as_str(), "value" | "checked" | "this"))
}

/// A place in a script, as a line and column of its file.
fn script_pos(s: &template::Script, off: usize) -> (u32, u32) {
    let before = &s.src[..off];
    let col = match before.rfind('\n') {
        Some(n) => before[n + 1..].chars().count() as u32 + 1,
        None => s.col + before.chars().count() as u32,
    };
    (s.line + before.matches('\n').count() as u32, col)
}

/// The components `nodes` has the browser render.
fn client_uses(t: &Template) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for d in t.groups.iter().flat_map(|g| &g.directives) {
        if d.kind == Dir::Comp && !out.contains(&d.name) {
            out.push(d.name.clone());
        }
    }
    out
}

/// The browser module of a template with browser code, or `None`.
fn client(t: &Tpl, cx: &ClientCx) -> Result<Option<Client>, String> {
    let tt = &t.t;
    if !tt.is_live() && !cx.as_client && cx.load.is_none() {
        return Ok(None);
    }
    let at = |line: u32, col: u32, msg: String| format!("{}:{line}:{col}: {msg}", t.rel);
    let server_load = matches!(t.user, Some((_, true)));
    let mut server: Vec<String> = match t.kind {
        // `data`, and each of its names alone (`items` for `data.items`),
        // but when a `+page.js` makes `data` in the browser.
        Kind::Page | Kind::Layout if server_load => {
            let mut names = vec!["data".to_string()];
            if cx.load.is_none() {
                let lets = t.stmts.as_ref().map(|(s, binds)| {
                    let mut n = rust_scan::let_names(s);
                    n.extend(rust_scan::let_names(&binds.join("\n")));
                    n
                });
                let fields = t.data.iter().map(|(n, _)| n.clone());
                for n in lets.unwrap_or_default().into_iter().chain(fields) {
                    let own = n.starts_with("__") || matches!(n.as_str(), "cx" | "children");
                    let js_own = js::is_reserved(&n) || js::is_global(&n);
                    if !own && !js_own && !names.contains(&n) {
                        names.push(n);
                    }
                }
            }
            names
        }
        Kind::Component => tt
            .props
            .iter()
            .flat_map(|(ds, _)| ds)
            .map(|d| d.name.clone())
            .collect(),
        _ => Vec::new(),
    };
    let script = tt.script.as_ref();
    let original = script.map_or("", |s| s.src.as_str());
    let script_at = |off: usize| script_pos(script.expect("an offset in the script"), off);
    let script_err = |(off, msg): (usize, String)| {
        let (line, col) = script_at(off);
        at(line, col, msg)
    };
    // A component's `let { a, b = 1 } = $props()` names props it reads,
    // with their browser defaults. Blanked, it declares nothing.
    let rune = js::props_rune(original).map_err(script_err)?;
    let blanked;
    let src = match &rune {
        Some(p) => {
            let (line, col) = script_at(p.span.0);
            if t.kind != Kind::Component {
                return Err(at(
                    line,
                    col,
                    "`$props()` is for components; a page's server values are `data`".into(),
                ));
            }
            if let Some(name) = p
                .props
                .iter()
                .map(|x| &x.name)
                .find(|n| !server.contains(n))
            {
                return Err(at(
                    line,
                    col,
                    format!(
                        "`{name}` is not a prop of this component: declare it in {{@props …}}, with its Rust type{}",
                        if server.is_empty() {
                            String::new()
                        } else {
                            format!(" (it has {})", server.join(", "))
                        }
                    ),
                ));
            }
            blanked = js::blank(original, &[p.span]);
            blanked.as_str()
        }
        None => original,
    };
    let declared = js::declarations(src);
    let imported = js::import_names(src, &js::imports(src));
    // A name a page's script declares is the script's, as a name its Rust
    // has too: `let guess = data.guess`.
    if t.kind != Kind::Component {
        server.retain(|n| {
            n == "data" || !(declared.iter().any(|(d, _)| d == n) || imported.contains(n))
        });
    }
    // A server name the script declares too could mean either.
    let clash = |name: &str| -> Result<(), String> {
        match declared.iter().find(|(n, _)| n == name) {
            Some((_, off)) => {
                let (line, col) = script_at(*off);
                Err(at(
                    line,
                    col,
                    format!("`{name}` is both a server value and a script variable; rename one"),
                ))
            }
            None => Ok(()),
        }
    };
    // What a component sends is each prop it reads, whole; a page's `data`
    // is sent a field at a time, or whole when a `+page.js` loads from it
    // (which the browser cannot know in advance).
    let sent = |path: &[String]| -> Vec<String> {
        if t.kind == Kind::Component || cx.load.is_some() {
            path[..1].to_vec()
        } else {
            server_path(path)
        }
    };

    // The server values read, in order, and where.
    let mut used: Vec<(Vec<String>, u32)> = Vec::new();
    for (path, off) in js::chains(src) {
        if server.contains(&path[0]) {
            clash(&path[0])?;
            used.push((sent(&path), script_at(off).0));
        }
    }
    if let Some(p) = &rune {
        let line = script_at(p.span.0).0;
        used.extend(p.props.iter().map(|x| (vec![x.name.clone()], line)));
        if p.rest.is_some() && server.iter().any(|s| s == REST) {
            used.push((vec![REST.into()], line));
        }
    }
    if cx.load.is_some() && server_load {
        used.push((
            vec!["data".into()],
            tt.script.as_ref().map_or(1, |s| s.line),
        ));
    }

    let mut scopes = vec![Vec::new(); tt.groups.len()];
    rust_scopes(&tt.nodes, &mut Vec::new(), &mut scopes);
    let own = cx.comps.iter().position(|c| c.module == t.module);
    let mut groups = Vec::new();
    let mut locals = Vec::new();
    let mut uses: Vec<usize> = Vec::new();
    for (g, scope) in tt.groups.iter().zip(&scopes) {
        let mut rust: Vec<(Vec<String>, u32)> = Vec::new();
        // The names a closure over `expr` takes from the element's locals:
        // client `each` names, then Rust ones.
        let mut names = |expr: &str, line: u32| -> Result<Vec<String>, String> {
            let mut names: Vec<String> = Vec::new();
            for (path, _) in js::chains(expr) {
                let root = &path[0];
                let local = if g.locals.contains(root) {
                    true
                } else if scope.contains(root) {
                    clash(root)?;
                    rust.push((server_path(&path), line));
                    true
                } else {
                    if server.contains(root) {
                        clash(root)?;
                        used.push((sent(&path), line));
                    }
                    false
                };
                if local && !names.contains(root) {
                    names.push(root.clone());
                }
            }
            Ok(names)
        };
        let mut bindings = Vec::new();
        for d in &g.directives {
            if d.kind == Dir::Comp {
                let (b, ci) = comp_binding(d, &mut names, cx).map_err(|m| at(d.line, d.col, m))?;
                // Outside any block that could stop it, it would never end.
                if Some(ci) == own && !g.nested {
                    return Err(at(
                        d.line,
                        d.col,
                        format!(
                            "<{0}> renders itself here with nothing to stop it; put it in a {{:#if}} or {{:#each}} that ends, \
                         such as {{:#each node.children as child}}<{0} node={{:child}} />{{/each}}",
                            d.name
                        ),
                    ));
                }
                if !uses.contains(&ci) && Some(ci) != own {
                    uses.push(ci);
                }
                bindings.push(b);
                continue;
            }
            bindings.push(binding(d, &mut names)?);
        }
        groups.push(bindings);
        locals.push(if rust.is_empty() {
            Vec::new()
        } else {
            json_tree(&rust)
        });
    }
    let imports: Vec<String> = uses.iter().map(|&ci| comp_placeholder(ci)).collect();

    // The server values or props the module takes, each with the variable
    // it is read as (`$props()` may rename one), and `...rest`'s.
    let local = |p: &str| {
        rune.as_ref()
            .and_then(|r| r.props.iter().find(|x| x.name == p))
            .map_or_else(|| p.to_string(), |x| x.local.clone())
    };
    let mut params: Vec<(String, String)> = Vec::new();
    let mut take = |name: &str| {
        if name != REST && !js::is_reserved(name) && !params.iter().any(|p| p.0 == name) {
            params.push((name.to_string(), local(name)));
        }
    };
    for (path, _) in &used {
        take(&path[0]);
    }
    // A component the browser renders takes every prop, from the page's
    // code; a `+page.js` hands the page its `data`.
    if cx.as_client {
        for name in &server {
            take(name);
        }
    }
    if cx.load.is_some() {
        take("data");
    }
    let rest = rune.as_ref().and_then(|r| r.rest.clone());
    // What the module runs: the script with its state as signals, and the
    // directives reading them as it does.
    let owned: Vec<String> = params
        .iter()
        .map(|p| p.1.clone())
        .chain(rest.clone())
        .collect();
    // `bind:value="q"` with no `q` anywhere declares it: `let q` at the end
    // of the script (or as the whole script), so offsets in it hold.
    let mut src = src.to_string();
    let mut bound: Vec<&str> = Vec::new();
    for (g, scope) in tt.groups.iter().zip(&scopes) {
        for d in g.directives.iter().filter(|d| d.kind == Dir::Bind) {
            let Some(v) = d.value.as_ref().map(|c| c.src.trim()) else {
                continue;
            };
            let free = ty::is_ident(v)
                && !js::is_reserved(v)
                && !bound.contains(&v)
                && !declared.iter().any(|(n, _)| n == v)
                && !imported.iter().any(|n| n == v)
                && !server.iter().any(|n| n == v)
                && !owned.iter().any(|n| n == v)
                && !g.locals.iter().any(|n| n == v)
                && !scope.iter().any(|n| n == v);
            if free {
                bound.push(v);
                src.push_str("\nlet ");
                src.push_str(v);
            }
        }
    }
    let src = src.as_str();
    let written: Vec<&str> = groups.iter().flatten().map(String::as_str).collect();
    let (runs, reactive) = js::script(src, &owned, cx.release, &written).map_err(script_err)?;
    for (bindings, g) in groups.iter_mut().zip(&tt.groups) {
        for b in bindings.iter_mut() {
            *b = js::rewrite(b, &reactive)
                .map_err(|(_, msg)| format!("{}:{}: {msg}", t.rel, g.line))?;
        }
    }
    let defaults: Vec<(String, String)> = rune
        .iter()
        .flat_map(|p| &p.props)
        .filter_map(|x| Some((x.name.clone(), x.default.clone()?)))
        .collect();
    let id = format!("t{}", t.id);
    let html = cx.as_client.then(|| {
        let mut s = String::new();
        client_html(&tt.nodes, tt, &mut s);
        s
    });
    let m = Module {
        id: &id,
        rel: &t.rel,
        params: &params,
        rest: rest.as_deref(),
        defaults: &defaults,
        script: (script.is_some() || !bound.is_empty())
            .then(|| (runs.as_str(), script.map_or(1, |s| s.line))),
        groups: &groups,
        imports: &imports,
        load: cx.load.as_deref(),
        extra: (tt.groups.iter().flat_map(|g| &g.directives).any(is_extra)
            || std::iter::once(runs.as_str())
                .chain(groups.iter().flatten().map(String::as_str))
                .any(|c| {
                    js::tokens(c).iter().any(|t| {
                        !t.member
                            && matches!(
                                t.text(c),
                                "Map" | "Set" | "enhance" | "__wisp_snap" | "persisted"
                            )
                    })
                }))
        .then_some(cx.extra),
        html: html.as_deref(),
        lib_hash: cx.lib_hash,
    };
    let source = module_source(&m);
    let hash = format!("{:016x}", fnv1a(source.as_bytes()));
    let blob = if used.is_empty() {
        vec![Piece::Text("{}".into())]
    } else {
        json_tree(&used)
    };
    // What the first paint may read: a `+page.js` changes `data` in the
    // browser, so the server does not know it.
    let mut known = server;
    known.retain(|n| !(cx.load.is_some() && n == "data"));
    let mut lets: Vec<(String, String)> = declared
        .iter()
        .filter_map(|(n, _)| {
            Some((
                n.clone(),
                js::plain_init(js::initializer(src, n)?)?.to_string(),
            ))
        })
        .collect();
    // A prop `$props()` renames is its prop, to the first paint.
    let mut declared: Vec<String> = declared.into_iter().map(|(n, _)| n).collect();
    for p in rune
        .iter()
        .flat_map(|r| &r.props)
        .filter(|p| p.local != p.name)
    {
        lets.push((p.local.clone(), p.name.clone()));
        declared.push(p.local.clone());
    }
    if let Some(r) = &rest {
        lets.push((r.clone(), REST.into()));
        declared.push(r.clone());
    }
    Ok(Some(Client {
        extra: m.extra.map(String::from),
        id,
        source,
        hash,
        blob,
        locals,
        uses,
        paints: cx.as_client,
        server: known,
        whole: t.kind == Kind::Component,
        declared,
        lets,
        scopes,
    }))
}

/// Where a module's import of component `ci`'s module goes, until the
/// URLs are known (see `generate`).
fn comp_placeholder(ci: usize) -> String {
    format!("@wisp/comp/{ci}")
}

/// `src` with the placeholder of each component `url` knows (`"@wisp/comp/3"`)
/// replaced by its module's URL, in one pass.
fn link_comps(src: &str, url: impl Fn(usize) -> Option<String>) -> String {
    const MARK: &str = "\"@wisp/comp/";
    let mut out = String::with_capacity(src.len());
    let mut rest = src;
    while let Some(i) = rest.find(MARK) {
        let after = &rest[i + MARK.len()..];
        let digits = after.bytes().take_while(u8::is_ascii_digit).count();
        let found = after[..digits].parse().ok().and_then(&url);
        match found.filter(|_| after[digits..].starts_with('"')) {
            Some(u) => {
                out.push_str(&rest[..i]);
                out.push_str(&js_str(&u));
                rest = &after[digits + 1..];
            }
            None => {
                out.push_str(&rest[..i + MARK.len()]);
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// The markup of a component the browser renders: its text, with every
/// directive element marked by its group alone.
fn client_html(nodes: &[Node], t: &Template, out: &mut String) {
    for n in nodes {
        match n {
            Node::Text(i) => out.push_str(&t.chunks[*i]),
            Node::Live { group } => {
                let _ = write!(out, " {GROUP_ATTR}=\"{group}\"");
            }
            Node::Client(branches) => {
                for (g, body) in branches {
                    let _ = write!(out, "<template {GROUP_ATTR}=\"{g}\">");
                    client_html(body, t, out);
                    out.push_str("</template>");
                }
            }
            Node::Render => {
                let _ = write!(out, "<template {SLOT_ATTR}></template>");
            }
            Node::Tag { .. } => out.push_str("wisp-element"),
            _ => {}
        }
    }
}

/// A module's parts, for `module_source`.
struct Module<'a> {
    id: &'a str,
    rel: &'a str,
    /// The server values or props, and the variable each is read as.
    params: &'a [(String, String)],
    /// `...rest`'s variable: the props not named.
    rest: Option<&'a str>,
    /// `$props()` defaults: a prop's JavaScript when it is not given.
    defaults: &'a [(String, String)],
    /// The script, as it runs, and the line of the file it starts on.
    script: Option<(&'a str, u32)>,
    groups: &'a [Vec<String>],
    /// Modules of the components it renders.
    imports: &'a [String],
    load: Option<&'a str>,
    /// `extra.js`'s URL, when the module uses it.
    extra: Option<&'a str>,
    html: Option<&'a str>,
    lib_hash: &'a str,
}

/// The module's text. The script keeps its line numbers, as far as the
/// lines before it allow, so the browser's errors point into the .wisp file.
///
/// The script runs in blocks of its own, inside the helpers and then the
/// server values (signals, which the runtime sets again when a morph or a
/// parent brings new ones), so it may reuse a helper's name and a server
/// value may too. Its function returns the binding groups.
fn module_source(m: &Module) -> String {
    let mut s = String::new();
    let _ = writeln!(
        s,
        "import {{ define }} from \"{LIVE_JS_PATH}?v={}\";",
        crate::runtime_version()
    );
    for url in m.imports {
        let _ = writeln!(s, "import {};", js_str(url));
    }
    if let Some(url) = m.load {
        let _ = writeln!(s, "import * as __wisp_u from {};", js_str(url));
    }
    if let Some(url) = m.extra {
        let _ = writeln!(s, "import {};", js_str(url));
    }
    let mut body = String::new();
    if let Some((src, _)) = m.script {
        // Imports go first, as a module's must; they leave blank lines.
        let spans = js::imports(src);
        for &(a, b) in &spans {
            s.push_str(&rewrite_specifiers(&src[a..b], m.lib_hash, None));
            s.push('\n');
        }
        body = js::blank(src, &spans);
    }
    let _ = write!(
        s,
        "define({}, function (__wisp_p, __wisp_h) {{ const {{ {HELPERS} }} = __wisp_h; {{ ",
        js_str(m.id)
    );
    if !m.params.is_empty() || m.rest.is_some() {
        let list: Vec<String> = m.params.iter().map(|p| js_str(&p.0)).collect();
        let mut names: Vec<String> = m
            .params
            .iter()
            .map(|(p, l)| {
                if p == l {
                    p.clone()
                } else {
                    format!("{p}: {l}")
                }
            })
            .collect();
        names.extend(m.rest.map(|r| format!("{REST}: {r}")));
        let defaults: Vec<String> = m
            .defaults
            .iter()
            .map(|(n, d)| format!("{n}: () => ({d})"))
            .collect();
        let tail = match (defaults.is_empty(), m.rest.is_some()) {
            (true, false) => String::new(),
            (_, rest) => format!(
                ", {{ {} }}{}",
                defaults.join(", "),
                if rest { ", 1" } else { "" }
            ),
        };
        let _ = write!(
            s,
            "const {{ {} }} = __wisp_props(__wisp_p, [{}]{tail}); ",
            names.join(", "),
            list.join(", "),
        );
    }
    s.push_str("{\n");
    if let Some((_, line)) = m.script {
        let next = s.matches('\n').count() as u32 + 1;
        for _ in next..line {
            s.push('\n');
        }
        s.push_str(&body);
        if !body.ends_with('\n') {
            s.push('\n');
        }
    }
    s.push_str("return { g: [\n");
    for g in m.groups {
        let _ = writeln!(s, "  [{}],", g.join(", "));
    }
    s.push_str("] };\n} } }");
    let mut opts = Vec::new();
    if let Some(h) = m.html {
        opts.push(format!("html: {}", js_str(h)));
    }
    if m.load.is_some() {
        opts.push("load: __wisp_u.load".into());
    }
    if !opts.is_empty() {
        let _ = write!(s, ", {{ {} }}", opts.join(", "));
    }
    s.push_str(");\n");
    let _ = writeln!(s, "//# sourceURL=wisp:///{}", m.rel);
    s
}

/// The URL the browser loads for `spec` in an import: `wisp` is the
/// runtime, `$lib/x.js` is `src/lib/x.js`, and in a lib file (whose
/// directory under `src/lib` is `base`) so is a relative path. Anything else
/// (a full URL) stays as written.
fn resolve_spec(spec: &str, lib_hash: &str, base: Option<&str>) -> Option<String> {
    if spec == "wisp" {
        return Some(format!("{LIVE_JS_PATH}?v={}", crate::runtime_version()));
    }
    let rel = if let Some(p) = spec.strip_prefix("$lib/") {
        p.to_string()
    } else {
        let b = base.filter(|_| spec.starts_with("./") || spec.starts_with("../"))?;
        let mut parts: Vec<&str> = b.split('/').filter(|p| !p.is_empty()).collect();
        for p in spec.split('/') {
            match p {
                "." | "" => {}
                ".." => {
                    parts.pop();
                }
                p => parts.push(p),
            }
        }
        parts.join("/")
    };
    Some(format!("{MODULES}lib/{rel}?v={lib_hash}"))
}

/// `src` with the module names of its imports (`import … from '…'`,
/// `export … from '…'`, `import('…')`) replaced as `resolve_spec` says. Every
/// file reaches a lib file by the same URL, so a store in it is one store.
fn rewrite_specifiers(src: &str, lib_hash: &str, base: Option<&str>) -> String {
    let t = js::tokens(src);
    let mut out = String::with_capacity(src.len());
    let mut at = 0;
    for k in 0..t.len() {
        if t[k].kind != js::Kind::String {
            continue;
        }
        let prev = |n: usize| k.checked_sub(n).map(|j| t[j].text(src));
        if !(matches!(prev(1), Some("from" | "import"))
            || (prev(1) == Some("(") && prev(2) == Some("import")))
        {
            continue;
        }
        let raw = t[k].text(src);
        if raw.len() < 2 {
            continue;
        }
        let Some(url) = resolve_spec(&raw[1..raw.len() - 1], lib_hash, base) else {
            continue;
        };
        out.push_str(&src[at..t[k].start]);
        out.push_str(&js_str(&url));
        at = t[k].end;
    }
    out.push_str(&src[at..]);
    out
}

/// Resolves the names an expression (with the line it is on) reads: the
/// element's locals, which its closure destructures, are returned.
type Names<'a> = dyn FnMut(&str, u32) -> Result<Vec<String>, String> + 'a;

/// An `on:` directive's modifiers as live.js takes them: bits, in the order
/// of `ON_FLAGS`, and `ON_ROOT` for an event handled by one listener at the root
/// (one that bubbles, and need not be where it is); then, when there are,
/// the keys (`KeyboardEvent.key` in lower case) and the debounce time in ms.
fn on_mods(event: &str, mods: &[String]) -> (u32, String) {
    const ROOTED: [&str; 14] = [
        "click",
        "dblclick",
        "input",
        "change",
        "keydown",
        "keyup",
        "pointerdown",
        "pointerup",
        "pointermove",
        "mousedown",
        "mouseup",
        "contextmenu",
        "focusin",
        "focusout",
    ];
    let (mut bits, mut keys, mut ms) = (0u32, Vec::new(), 0u32);
    let mut k = 0;
    while k < mods.len() {
        let m = mods[k].as_str();
        if let Some(i) = ON_FLAGS.iter().position(|f| *f == m) {
            bits |= 1 << i;
        } else if m == "debounce" {
            ms = 250;
            let time = mods.get(k + 1).and_then(|t| match t.strip_suffix("ms") {
                Some(n) => n.parse::<u32>().ok(),
                None => t.strip_suffix('s')?.parse::<u32>().ok().map(|n| n * 1000),
            });
            if let Some(t) = time {
                ms = t;
                k += 1;
            }
        } else {
            keys.push(js_str(match m {
                "space" => " ",
                "up" => "arrowup",
                "down" => "arrowdown",
                "left" => "arrowleft",
                "right" => "arrowright",
                key => key,
            }));
        }
        k += 1;
    }
    // capture, passive, window, document, outside: where it is.
    if ROOTED.contains(&event) && bits & ON_PLACED == 0 {
        bits |= ON_ROOT;
    }
    let rest = match (keys.is_empty(), ms) {
        (true, 0) => String::new(),
        (true, ms) => format!(", null, {ms}"),
        (false, 0) => format!(", [{}]", keys.join(", ")),
        (false, ms) => format!(", [{}], {ms}", keys.join(", ")),
    };
    (bits, rest)
}

/// `(L) =>` and `(L, x) =>`, destructuring what the expression reads.
fn one(n: &[String]) -> String {
    if n.is_empty() {
        "()".to_string()
    } else {
        format!("({{ {} }})", n.join(", "))
    }
}

fn two(n: &[String], x: &str) -> String {
    if n.is_empty() {
        format!("(_, {x})")
    } else {
        format!("({{ {} }}, {x})", n.join(", "))
    }
}

/// An expression as an arrow's body. The newline keeps a trailing `//`
/// comment from swallowing the `)`.
fn paren(e: &str) -> String {
    format!(
        "({e}{})",
        if js::ends_in_line_comment(e) {
            "\n"
        } else {
            ""
        }
    )
}

/// An event handler's body: a plain path is called with the event, one
/// expression is returned (so a promise is seen), and statements are a block.
fn handler_body(src: &str) -> String {
    if js::is_path(src) {
        format!("{src}(event)")
    } else if js::is_statements(src) {
        format!(
            "{{ {src}{} }}",
            if js::ends_in_line_comment(src) {
                "\n"
            } else {
                ""
            }
        )
    } else {
        paren(src)
    }
}

/// One directive as the runtime takes it (see the contract in `live.js`).
/// `names` resolves an expression's locals, which its closure destructures.
fn binding(d: &Directive, names: &mut Names) -> Result<String, String> {
    let getter = |c: &Code, names: &mut Names| -> Result<String, String> {
        Ok(format!(
            "{} => {}",
            one(&names(&c.src, c.line)?),
            paren(&c.src)
        ))
    };
    let optional = |c: Option<&Code>, names: &mut Names| -> Result<String, String> {
        match c {
            Some(c) => getter(c, names),
            None => Ok("null".into()),
        }
    };
    let value = || {
        d.value
            .as_ref()
            .expect("the parser checked that it has a value")
    };
    let name = js_str(&d.name);
    Ok(match d.kind {
        Dir::On => {
            let c = value();
            let n = names(&c.src, c.line)?;
            let (bits, rest) = on_mods(&d.name, &d.mods);
            format!(
                "[\"on\", {name}, {bits}, {} => {}{rest}]",
                two(&n, "event"),
                handler_body(&c.src)
            )
        }
        Dir::Bind => {
            let c = value();
            let n = names(&c.src, c.line)?;
            let get = if d.name == "this" {
                "null".to_string()
            } else {
                format!("{} => {}", one(&n), paren(&c.src))
            };
            format!(
                "[\"bind\", {name}, {get}, {} => {{ {} = __wisp_v }}]",
                two(&n, "__wisp_v"),
                paren(&c.src)
            )
        }
        Dir::Attr => format!("[\"attr\", {name}, {}]", getter(value(), names)?),
        Dir::Text => format!("[\"text\", {}]", getter(value(), names)?),
        Dir::Hole => format!("[\"hole\", {}]", getter(value(), names)?),
        Dir::Class => format!("[\"class\", {name}, {}]", getter(value(), names)?),
        Dir::Style => format!("[\"style\", {name}, {}]", getter(value(), names)?),
        // A built-in by its name, else the script's function; then 0 both
        // ways, 1 in, 2 out.
        Dir::Transition => {
            let kind = if matches!(d.name.as_str(), "fade" | "slide" | "scale" | "fly" | "blur") {
                name
            } else {
                format!("{} => {}", one(&names(&d.name, d.line)?), d.name)
            };
            let dir = match d.mods.first().map(String::as_str) {
                Some("in") => 1,
                Some("out") => 2,
                _ => 0,
            };
            format!(
                "[\"transition\", {kind}, {}, {dir}]",
                optional(d.value.as_ref(), names)?
            )
        }
        Dir::Animate => format!(
            "[\"animate\", {name}, {}]",
            optional(d.value.as_ref(), names)?
        ),
        Dir::Use => {
            let f = format!("{} => {}", one(&names(&d.name, d.line)?), d.name);
            format!("[\"use\", {f}, {}]", optional(d.value.as_ref(), names)?)
        }
        Dir::Each => {
            let own: Vec<String> = std::iter::once(&d.name).chain(&d.mods).cloned().collect();
            let key = match &d.key {
                Some(k) => {
                    let mut n = own.clone();
                    for x in names(&k.src, k.line)? {
                        if !n.contains(&x) {
                            n.push(x);
                        }
                    }
                    format!("{} => {}", one(&n), paren(&k.src))
                }
                None => "null".into(),
            };
            let own: Vec<String> = own.iter().map(|n| js_str(n)).collect();
            format!(
                "[\"each\", {}, [{}], {key}]",
                getter(value(), names)?,
                own.join(", ")
            )
        }
        Dir::If => format!("[\"if\", {}]", getter(value(), names)?),
        Dir::Key => format!("[\"key\", {}]", getter(value(), names)?),
        Dir::Await => format!("[\"await\", {}]", getter(value(), names)?),
        Dir::Try => "[\"try\"]".into(),
        Dir::Spread => format!("[\"spread\", {}]", getter(value(), names)?),
        Dir::Tag => format!("[\"tag\", {}]", getter(value(), names)?),
        Dir::At | Dir::Wait => format!(
            "[\"{}\", {name}]",
            if d.kind == Dir::At { "at" } else { "wait" }
        ),
        Dir::Comp => unreachable!("comp_binding builds it"),
    })
}

/// `<Card title={:x} bind:open="o" on:select="pick">` where the browser
/// renders it: `["comp", ID, (L) => props, binds, events]`, and the
/// component's index, whose module the page's module imports.
fn comp_binding(
    d: &Directive,
    names: &mut Names,
    cx: &ClientCx,
) -> Result<(String, usize), String> {
    let name = &d.name;
    let Some(ci) = cx.comps.iter().position(|c| c.name == *name) else {
        let known: Vec<&str> = cx.comps.iter().map(|c| c.name.as_str()).collect();
        return Err(format!(
            "no component `{name}`: components are the .wisp files in src/components, and there are {}",
            if known.is_empty() {
                "none yet".into()
            } else {
                known.join(", ")
            }
        ));
    };
    let c = &cx.comps[ci];
    if !template::client_renderable(&cx.templates[ci].t.nodes) {
        return Err(format!(
            "<{name}> is rendered in the browser here, but its markup has server code ({{…}} or a {{#…}} block). \
             In a component the browser renders, show props with {{:prop}} and use {{:#if}} and {{:#each}}"
        ));
    }
    let mut all: Vec<String> = Vec::new();
    let mut add = |n: Vec<String>| {
        for x in n {
            if !all.contains(&x) {
                all.push(x);
            }
        }
    };
    let (mut props, mut binds, mut events) = (Vec::new(), Vec::new(), Vec::new());
    for p in &d.props {
        // `{:...obj}`: its keys, where it stands among the others.
        if let (true, PropValue::Live(code)) = (p.name == "...", &p.value) {
            add(names(&code.src, code.line)?);
            props.push(format!("...{}", paren(&code.src)));
            continue;
        }
        let key = js_str(&p.name);
        let declared = c.props.iter().any(|x| x.name == p.name);
        if !declared && !c.rest && !matches!(p.value, PropValue::On(_)) {
            let takes: Vec<&str> = c.props.iter().map(|x| x.name.as_str()).collect();
            return Err(format!(
                "<{name}> has no prop `{}`; it takes {}",
                p.name,
                if takes.is_empty() {
                    "none".into()
                } else {
                    takes.join(", ")
                }
            ));
        }
        match &p.value {
            PropValue::Text(t) => props.push(format!("{key}: {}", js_str(t))),
            PropValue::Flag => props.push(format!("{key}: true")),
            PropValue::Live(code) => {
                add(names(&code.src, code.line)?);
                props.push(format!("{key}: {}", paren(&code.src)));
            }
            PropValue::Bind(code) => {
                if c.bindable.as_ref().is_some_and(|b| !b.contains(&p.name)) {
                    return Err(format!(
                        "<{name}>'s `{0}` is not bindable: its script marks the props a parent may bind, \
                         `let {{ {0} = $bindable() }} = $props()`",
                        p.name
                    ));
                }
                let n = names(&code.src, code.line)?;
                binds.push(format!(
                    "[{key}, {} => {}, {} => {{ {} = __wisp_v }}]",
                    one(&n),
                    paren(&code.src),
                    two(&n, "__wisp_v"),
                    paren(&code.src)
                ));
                add(n);
                props.push(format!("{key}: {}", paren(&code.src)));
            }
            PropValue::On(code) => {
                let n = names(&code.src, code.line)?;
                events.push(format!(
                    "[{key}, {} => {}]",
                    two(&n, "event"),
                    handler_body(&code.src)
                ));
            }
            PropValue::Expr(_) | PropValue::Snippet { .. } => {
                unreachable!("the parser refuses server props here")
            }
        }
    }
    let b = format!(
        "[\"comp\", {}, {} => ({{ {} }}), [{}], [{}]]",
        js_str(&format!("t{}", cx.templates[ci].id)),
        one(&all),
        props.join(", "),
        binds.join(", "),
        events.join(", ")
    );
    Ok((b, ci))
}

/// The part of a chain read in the browser that the server sends: its Rust
/// field names, without a last `length` (a list or a string arrives whole,
/// and JavaScript knows its length).
fn server_path(path: &[String]) -> Vec<String> {
    let mut p: Vec<String> = path
        .iter()
        .take_while(|s| ty::is_ident(s))
        .cloned()
        .collect();
    if p.len() > 1 && p.last().is_some_and(|l| l == "length") {
        p.pop();
    }
    p
}

/// `data.type` is `data.r#type` in Rust.
fn rust_place(path: &[String]) -> String {
    const KEYWORDS: [&str; 38] = [
        "as", "async", "await", "break", "const", "continue", "dyn", "else", "enum", "extern",
        "false", "fn", "for", "gen", "if", "impl", "in", "let", "loop", "match", "mod", "move",
        "mut", "pub", "ref", "return", "static", "struct", "trait", "true", "try", "type",
        "unsafe", "use", "where", "while", "yield", "box",
    ];
    let segs: Vec<String> = path
        .iter()
        .map(|s| {
            if KEYWORDS.contains(&s.as_str()) {
                format!("r#{s}")
            } else {
                s.clone()
            }
        })
        .collect();
    segs.join(".")
}

/// A JSON object of the values at `paths` (each with where it is read),
/// nested as they are: `data.a.b` and `data.c` are `{"data":{"a":{"b":…},
/// "c":…}}`. A path under another is covered by it.
fn json_tree(paths: &[(Vec<String>, u32)]) -> Vec<Piece> {
    let mut keep: Vec<&(Vec<String>, u32)> = Vec::new();
    for p in paths {
        let covered = paths
            .iter()
            .any(|q| q.0.len() < p.0.len() && p.0.starts_with(&q.0))
            || keep.iter().any(|q| q.0 == p.0);
        if !covered {
            keep.push(p);
        }
    }
    let mut out = Vec::new();
    json_object(&keep, 0, &mut out);
    // Adjacent text as one piece.
    let mut merged: Vec<Piece> = Vec::new();
    for p in out {
        match (merged.last_mut(), p) {
            (Some(Piece::Text(a)), Piece::Text(b)) => a.push_str(&b),
            (_, p) => merged.push(p),
        }
    }
    merged
}

fn json_object(paths: &[&(Vec<String>, u32)], depth: usize, out: &mut Vec<Piece>) {
    out.push(Piece::Text("{".into()));
    let mut keys: Vec<&str> = Vec::new();
    for p in paths {
        let key = p.0[depth].as_str();
        if keys.contains(&key) {
            continue;
        }
        if !keys.is_empty() {
            out.push(Piece::Text(",".into()));
        }
        keys.push(key);
        out.push(Piece::Text(format!("{}:", js_str(key))));
        let under: Vec<&(Vec<String>, u32)> = paths
            .iter()
            .filter(|q| q.0[depth] == key)
            .copied()
            .collect();
        match under.as_slice() {
            [one] if one.0.len() == depth + 1 => out.push(Piece::Value {
                expr: rust_place(&one.0),
                line: one.1,
            }),
            _ => json_object(&under, depth + 1, out),
        }
    }
    out.push(Piece::Text("}".into()));
}

/// The Rust names bound around each `Live` node: by `{#each}`, `if let`,
/// `{:case}` and `{@const}`.
fn rust_scopes(nodes: &[Node], scope: &mut Vec<String>, out: &mut [Vec<String>]) {
    let outer = scope.len();
    for n in nodes {
        match n {
            Node::Const(c) => {
                // `{@const x: T = e}`: the pattern is before `=`, and before a type.
                scope.extend(pattern_names(template::untyped(let_pattern(&c.src))));
            }
            Node::Snippet { params, body, .. } => {
                let k = scope.len();
                for p in params {
                    scope.extend(pattern_names(template::untyped(p)));
                }
                rust_scopes(body, scope, out);
                scope.truncate(k);
            }
            Node::Live { group } => out[*group] = scope.clone(),
            Node::If {
                branches,
                otherwise,
            } => {
                for (cond, body) in branches {
                    let k = scope.len();
                    if let Some(rest) = cond
                        .src
                        .strip_prefix("let")
                        .filter(|r| r.starts_with(char::is_whitespace))
                    {
                        scope.extend(pattern_names(let_pattern(rest)));
                    }
                    rust_scopes(body, scope, out);
                    scope.truncate(k);
                }
                if let Some(o) = otherwise {
                    rust_scopes(o, scope, out);
                }
            }
            Node::Each {
                pat,
                index,
                body,
                otherwise,
                ..
            } => {
                let k = scope.len();
                scope.extend(index.iter().cloned());
                scope.extend(pattern_names(pat));
                rust_scopes(body, scope, out);
                scope.truncate(k);
                if let Some(o) = otherwise {
                    rust_scopes(o, scope, out);
                }
            }
            Node::Match { arms, .. } => {
                for (pat, body) in arms {
                    let k = scope.len();
                    scope.extend(pattern_names(guardless(&pat.src)));
                    rust_scopes(body, scope, out);
                    scope.truncate(k);
                }
            }
            Node::Head(body)
            | Node::Component {
                children: Some(body),
                ..
            } => rust_scopes(body, scope, out),
            // Its names end with it, as a block's do.
            Node::Client(branches) => {
                for (group, body) in branches {
                    out[*group] = scope.clone();
                    let k = scope.len();
                    rust_scopes(body, scope, out);
                    scope.truncate(k);
                }
            }
            _ => {}
        }
    }
    scope.truncate(outer);
}

/// The macro a `{#snippet}` of this name compiles to.
fn snippet_macro(name: &str) -> String {
    format!("__wisp_snippet_{name}")
}

/// `PAT = EXPR` → `PAT`.
fn let_pattern(s: &str) -> &str {
    let b = s.as_bytes();
    let mut eq = None;
    template::for_each_top(s, |i| {
        let plain = b[i] == b'='
            && b.get(i + 1).is_none_or(|&c| c != b'=' && c != b'>')
            && (i == 0 || !matches!(b[i - 1], b'=' | b'!' | b'<' | b'>' | b'.'));
        if plain && eq.is_none() {
            eq = Some(i);
        }
    });
    s[..eq.unwrap_or(s.len())].trim()
}

/// A `{:case}` pattern without its `if` guard.
fn guardless(pat: &str) -> &str {
    let b = pat.as_bytes();
    let mut at = None;
    template::for_each_top(pat, |i| {
        let word = b[i..].starts_with(b"if")
            && i > 0
            && b[i - 1].is_ascii_whitespace()
            && b.get(i + 2).is_some_and(|c| c.is_ascii_whitespace());
        if word && at.is_none() {
            at = Some(i);
        }
    });
    pat[..at.unwrap_or(pat.len())].trim()
}

/// The names a Rust pattern binds: `(k, v)`, `Some(x)`, `Point { x, y: py }`,
/// `n @ 1..=5`. Paths, constructors, field names and literals are not names.
fn pattern_names(pat: &str) -> Vec<String> {
    let b = pat.as_bytes();
    let mut out: Vec<String> = Vec::new();
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        if c == b'"' {
            i = template::skip_str(b, i) + 1;
            continue;
        }
        if c == b'\'' {
            i = template::skip_char(b, i) + 1;
            continue;
        }
        if !ty::is_word(c) {
            i += 1;
            continue;
        }
        let start = i;
        i = rust_scan::ident_end(b, i);
        let word = &pat[start..i];
        let (before, after) = (pat[..start].trim_end(), pat[i..].trim_start());
        let lower =
            word.as_bytes()[0].is_ascii_lowercase() || (word.starts_with('_') && word.len() > 1);
        let binds = lower
            && !matches!(word, "ref" | "mut" | "box" | "true" | "false")
            && !before.ends_with("::")
            && !after.starts_with("::")
            && !after.starts_with(['(', '{', '!'])
            && !(after.starts_with(':') && !after.starts_with("::"));
        if binds && !out.iter().any(|o| o == word) {
            out.push(word.to_string());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn field_paths() {
        assert!(is_field_path("data.posts"));
        assert!(is_field_path("p.tags.0"));
        assert!(!is_field_path("posts"));
        assert!(!is_field_path("data.posts.iter()"));
        assert!(!is_field_path("0..n"));
    }

    #[test]
    fn let_conditions_borrow_places() {
        let locals = ["user".to_string()];
        let cond = |c: &str| if_condition(c, &locals);
        assert_eq!(
            cond("let Some(u) = data.user"),
            "let Some(u) = &(data.user)"
        );
        assert_eq!(cond("let Some(u) = user"), "let Some(u) = &(user)");
        assert_eq!(cond("let Some(u) = find(x)"), "let Some(u) = find(x)");
        assert_eq!(cond("let 1..=5 = n"), "let 1..=5 = n");
        assert_eq!(cond("a == b"), "a == b");
        assert_eq!(cond("letter"), "letter");
        assert_eq!(
            rust_scan::let_names(
                "let (a, mut b) = x;\nif c { let d = 1; }\nlet Some(e) = f else { return };\nlet g: Vec<u8> = h;"
            ),
            ["a", "b", "e", "g"]
        );
    }

    /// Generates the app made of `files` (path, contents), in a scratch
    /// directory: the error if it fails, `Ok` with the code otherwise.
    fn app(name: &str, files: &[(&str, &str)]) -> Result<String, String> {
        build(name, files, false)
    }

    /// `app`, as a release build or a dev one.
    fn build(name: &str, files: &[(&str, &str)], release: bool) -> Result<String, String> {
        in_dir(name, files, |root| generate(&Input { root, release }))
    }

    /// The model of the app made of `files`, or the error reading it.
    fn model(name: &str, files: &[(&str, &str)]) -> Result<Model, String> {
        in_dir(name, files, |root| {
            Project::load(&Input {
                root,
                release: false,
            })
            .map(|p| p.model)
        })
    }

    /// `f` of a scratch directory holding `files` (path, contents).
    fn in_dir<T>(name: &str, files: &[(&str, &str)], f: impl FnOnce(&Path) -> T) -> T {
        let root = std::env::temp_dir().join(format!("wisp-codegen-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        for (path, contents) in files {
            let p = root.join(path);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(p, contents).unwrap();
        }
        let out = f(&root);
        let _ = fs::remove_dir_all(&root);
        out
    }

    /// Each route's pattern and whether a request through it may wait.
    fn waits(m: &Model) -> Vec<(&str, bool)> {
        m.routes
            .iter()
            .map(|r| (r.pattern.as_str(), m.route_waits(r)))
            .collect()
    }

    /// `field` of each row of the generated `App::ROUTES`, as written.
    fn facts<'a>(code: &'a str, field: &str) -> Vec<&'a str> {
        let field = format!("{field}: ");
        (code.lines())
            .filter(|l| l.trim_start().starts_with("::wisp::rt::RouteFacts {"))
            .map(|l| {
                let v = &l[l.find(&field).unwrap() + field.len()..];
                &v[..v.find([',', ' ']).unwrap()]
            })
            .collect()
    }

    #[test]
    fn a_route_knows_whether_a_file_may_be_at_its_paths() {
        let s = |x: &str| Seg::Static(x.into());
        let (user, param, rest) = (
            s("user"),
            Seg::Param("id".into(), None),
            Seg::Rest("r".into()),
        );
        assert!(may_match(&[&user, &param], "/user/a.png"));
        assert!(!may_match(&[&user, &param], "/static/app.js"));
        assert!(!may_match(&[&user, &param], "/user/a/b.png"));
        assert!(!may_match(&[&user], "/user/a.png"));
        assert!(may_match(&[&s("a b")], "/a%20b"));
        assert!(may_match(&[&user, &rest], "/x.css"));
        assert!(!may_match(&[], "/x.css"));
    }

    #[test]
    fn routes_that_never_wait_are_answered_now() {
        let files = [
            ("src/routes/+page.wisp", "<p>hi</p>"),
            ("src/routes/a/+page.wisp", "<p>{data.n}</p>"),
            (
                "src/routes/a/+page.rs",
                "pub struct Data { pub n: u8 }\npub async fn load() -> Data { Data { n: 1 } }",
            ),
            (
                "src/routes/b/+server.rs",
                "pub fn get(id: u64) -> String { id.to_string() }",
            ),
            ("src/routes/c/+page.wisp", "<p>{db::n().await}</p>"),
            (
                "src/routes/d/+page.wisp",
                "---\nlet n = 1;\n---\n<p>{n}</p>",
            ),
        ];
        let m = model("now", &files).unwrap();
        assert_eq!(
            waits(&m),
            [
                ("/", false),
                ("/a", true),
                ("/b/[id=int]", false),
                ("/c", true),
                ("/d", false)
            ]
        );
        assert!(m.root_error.is_none() && !m.root_waits());
        // The table `handle`'s server reads, and unmatched paths.
        let code = app("now", &files).unwrap();
        assert_eq!(
            facts(&code, "now"),
            ["true", "false", "true", "false", "true"]
        );
        assert!(code.contains("const NOT_FOUND_NOW: bool = true;"), "{code}");
        // A `before` that waits is before every route.
        let mut hooked = files.to_vec();
        hooked.push(("src/hooks.rs", "pub async fn before(cx: &mut wisp::Cx) {}"));
        let code = app("now-hooked", &hooked).unwrap();
        assert_eq!(facts(&code, "now"), ["false"; 5]);
        assert!(!code.contains("NOT_FOUND_NOW"), "{code}");
        // An error page whose layout loads with `.await`: its routes wait,
        // and so does a path no route matches.
        let mut errors = files.to_vec();
        errors.push(("src/routes/+error.wisp", "<p>{status}</p>"));
        errors.push(("src/routes/+layout.wisp", "<slot />"));
        errors.push((
            "src/routes/+layout.rs",
            "pub struct Data;\npub async fn load() -> Data { Data }",
        ));
        let m = model("now-error", &errors).unwrap();
        assert!(m.routes.iter().all(|r| m.route_waits(r)));
        assert!(m.root_error == Some(0) && m.root_waits());
        assert!(m.layouts[0].waits && m.error_waits(0) && m.errors[0].layouts == [0]);
        let code = app("now-error", &errors).unwrap();
        assert!(!code.contains("NOT_FOUND_NOW"), "{code}");
        assert_eq!(facts(&code, "error"), ["Some(0)"; 5]);
    }

    #[test]
    fn routes_that_may_wait_are_never_now() {
        let waits = |name: &str, files: &[(&str, &str)]| -> Vec<bool> {
            let m = model(name, files).unwrap();
            m.routes.iter().map(|r| m.route_waits(r)).collect()
        };
        let server = |src| [("src/routes/+server.rs", src)];
        // Awaits a macro may hide, and one spaced out.
        for (n, block) in [
            "let (a, b) = tokio::join!(f(), g());",
            "let a = get!(f());",
            "let a = f(). await;",
            "let a = f()./* c */await;",
        ]
        .iter()
        .enumerate()
        {
            let src = format!("---\n{block}\n---\n<p>{{a}}</p>");
            let files = [("src/routes/+page.wisp", src.as_str())];
            assert_eq!(waits(&format!("now-macro-{n}"), &files), [true], "{block}");
        }
        // An `async fn` in the module, even a helper.
        let helper = "async fn h() {}\npub fn get() -> String { String::new() }";
        assert_eq!(waits("now-helper", &server(helper)), [true]);
        // A plain `fn` whose response awaits later, in a task of its own;
        // std's macros, which cannot await; an async `init`, which runs
        // before any request.
        let ws = "fn get() -> Response { Response::websocket(|ws| async move { while let Some(m) = ws.recv().await { ws.send(m).await?; } Ok(()) }) }";
        let mut files = server(ws).to_vec();
        files.push(("src/hooks.rs", "async fn init() -> Result { Ok(()) }"));
        files.push((
            "src/routes/a/+page.wisp",
            "---\nlet a = format!(\"{}\", vec![1].len());\n---\n<p>{a}</p>",
        ));
        assert_eq!(waits("now-plain", &files), [false, false]);
        let code = app("now-plain", &files).unwrap();
        assert_eq!(facts(&code, "now"), ["true", "true"]);
    }

    #[test]
    fn layouts_and_error_pages_resolve_per_route() {
        let m = model(
            "chains",
            &[
                ("src/routes/+layout.wisp", "<slot />"),
                ("src/routes/+error.wisp", "{status}"),
                ("src/routes/+page.wisp", "x"),
                ("src/routes/blog/+layout.wisp", "<slot />"),
                (
                    "src/routes/blog/+layout.rs",
                    "struct Data;\nfn load() -> Data { Data }",
                ),
                ("src/routes/blog/[slug]/+page.wisp", "{slug}"),
                ("src/routes/blog/[slug]/+error.wisp", "{status}"),
                ("src/routes/docs/+page.wisp", "x"),
                ("src/components/Card.wisp", "x"),
            ],
        )
        .unwrap();
        let shape: Vec<(&str, &[usize], Option<usize>)> = (m.routes.iter())
            .map(|r| (r.pattern.as_str(), &r.layouts[..], r.error))
            .collect();
        assert_eq!(
            shape,
            [
                ("/", &[0][..], Some(0)),
                ("/blog/[slug]", &[0, 1], Some(1)),
                ("/docs", &[0], Some(0))
            ]
        );
        // Each layout and error page knows its own template, whatever else
        // comes before it (template 0 is the component).
        assert_eq!(
            m.layouts
                .iter()
                .map(|l| (l.tpl, l.load))
                .collect::<Vec<_>>(),
            [(1, false), (2, true)]
        );
        assert_eq!(m.errors.iter().map(|e| e.tpl).collect::<Vec<_>>(), [3, 4]);
        assert_eq!(
            (m.root_error, &m.errors[1].layouts[..]),
            (Some(0), &[0, 1][..])
        );
        let page = m.routes[1].page.as_ref().unwrap();
        assert_eq!((page.module.as_str(), page.tpl), ("page_1", 6));
    }

    #[test]
    fn component_uses_are_checked() {
        let card = (
            "src/components/Card.wisp",
            "{@props title: &str, big: bool = false}\n<h2>{title}</h2>{@render children()}",
        );
        let badge = ("src/components/Badge.wisp", "<b>new</b>");
        let page = |src: &'static str| ("src/routes/+page.wisp", src);
        assert!(
            app(
                "ok",
                &[card, badge, page("<Card title=\"x\" big><Badge /></Card>")]
            )
            .is_ok()
        );
        let err = |name, src| app(name, &[card, badge, page(src)]).unwrap_err();
        assert!(
            err("unknown", "\n<Crad title=\"x\" />")
                .starts_with("src/routes/+page.wisp:2: no component `Crad`")
        );
        assert!(err("unknown", "<Crad />").contains("there are Badge, Card"));
        assert!(err("missing", "<Card />").contains("<Card> needs `title`"));
        assert!(
            err("extra", "<Card title=\"x\" titel=\"y\" />")
                .contains("no prop `titel`; it takes title, big")
        );
        assert!(err("flag", "<Card title />").contains("`title` alone means true"));
        assert!(err("children", "<Badge>hi</Badge>").contains("does not show children"));
        assert!(app("blank", &[badge, page("<Badge>\n</Badge>")]).is_ok());
        assert!(
            err("head", "<wisp:head><Badge /></wisp:head>").contains("cannot go in <wisp:head>")
        );
        assert!(
            app("name", &[("src/components/card.wisp", "x"), page("x")])
                .unwrap_err()
                .contains("such as Card.wisp")
        );
        assert!(
            app("caps", &[("src/components/UI.wisp", "x"), page("x")])
                .unwrap_err()
                .contains("such as Ui.wisp")
        );
        assert!(
            app("props", &[page("{@props a: u8}")])
                .unwrap_err()
                .contains("only components")
        );
    }

    #[test]
    fn markup_awaits() {
        let page = |src: &'static str| ("src/routes/+page.wisp", src);
        let code = app(
            "await-ok",
            &[page(
                "---\nlet n = 1;\n---\n<h1>{db::title().await}</h1>\n\n{#each db::items(n)\n  .await as i}{i}{/each}",
            )],
        )
        .unwrap();
        for want in [
            "        let n = 1; // src/routes/+page.wisp:2",
            " let __wisp_a0 = db::title().await; // src/routes/+page.wisp:4",
            " let __wisp_a1 = db::items(n) // src/routes/+page.wisp:6",
            "  .await; // src/routes/+page.wisp:7",
            "__wisp_a0",
            "__wisp_a1",
        ] {
            assert!(code.contains(want), "{want}\n{code}");
        }
        // No block: the page still awaits, before it renders.
        let bare = app("await-bare", &[page("<p>{db::n().await}</p>")]).unwrap();
        assert!(
            bare.contains("let __wisp_a0 = db::n().await; // src/routes/+page.wisp:1"),
            "{bare}"
        );
        let err = |name, files: &[(&str, &str)]| app(name, files).unwrap_err();
        assert_eq!(
            err("await-in", &[page("{#if ok}\n{db::n().await}{/if}")]),
            "src/routes/+page.wisp:2: `.await` in markup runs before the page renders, so it goes outside any block \
             (`{#each db::items().await as item}`); inside one, await in the `---` block and name the value"
        );
        assert!(
            err(
                "await-load",
                &[
                    page("{db::n().await}"),
                    (
                        "src/routes/+page.rs",
                        "struct Data;\nfn load() -> Data { Data }"
                    )
                ]
            )
            .starts_with(
                "src/routes/+page.wisp:1: `.await` in markup runs with the page's statements"
            )
        );
        assert!(
            err(
                "await-layout",
                &[("src/routes/+layout.wisp", "{db::n().await}<slot />")]
            )
            .starts_with("src/routes/+layout.wisp:1: a layout renders without waiting")
        );
        assert!(
            err(
                "await-comp",
                &[(
                    "src/components/Card.wisp",
                    "{@props n: u8}\n<p>{db::n().await}</p>"
                )]
            )
            .starts_with("src/components/Card.wisp:2: a component renders without waiting")
        );
        // In a string, `.await` is text.
        let text = app("await-text", &[page("<p>{\"x.await\"}</p>")]).unwrap();
        assert!(!text.contains("__wisp_a0"), "{text}");
    }

    #[test]
    fn server_names_in_browser_code() {
        let code = app(
            "bare-names",
            &[(
                "src/routes/+page.wisp",
                "---\nlet items = vec![1u8];\nlet guess = 2u8;\n---\n\
                 <p :text=\"items.length\"></p><p :text=\"data.guess\"></p>\n\
                 <script>let guess = data.guess\nlet n = items[0]</script>",
            )],
        )
        .unwrap();
        // `items` is sent under its own name; the script's `guess` is its
        // own, and `data.guess` the server's.
        for want in [
            "::wisp::rt::json(__b, &(items));",
            "const { data, items } = __wisp_props(__wisp_p, [\\\"data\\\", \\\"items\\\"])",
            "let guess = __wisp_s(data.v.guess)",
        ] {
            assert!(code.contains(want), "{want}\n{code}");
        }
    }

    #[test]
    fn helpers_come_from_the_runtime() {
        // `matches` needs no import: every module's function is handed it.
        let live = wisp_shared::LIVE_JS;
        assert!(HELPERS.contains(" matches,"));
        assert!(live.contains("export const matches = ") && live.contains("  matches,"));
    }

    #[test]
    fn page_blocks() {
        let page = |src: &'static str| ("src/routes/blog/[slug]/+page.wisp", src);
        let code = app(
            "block-ok",
            &[
                page("---\nlet n = slug.len();\n\n#[action]\nfn like() { cx.flash(\"x\"); }\nlet s = \"a\nb\";\n---\n<p>{n}{#each list as x}{x}{/each}</p>"),
                ("src/routes/+layout.wisp", "---\nconst A: u8 = 1;\nlet p = cx.path();\n---\n{@render children()}"),
                ("src/db.rs", "pub fn f() {}"),
                ("src/main.rs", "mod own;\nwisp::main!();"),
                ("src/own.rs", ""),
            ],
        )
        .unwrap();
        for want in [
            "let slug = cx.param(\"slug\").to_string();",
            "    let n = slug.len(); // src/routes/blog/[slug]/+page.wisp:2",
            "    fn like() { cx.flash(\"x\"); } // src/routes/blog/[slug]/+page.wisp:5",
            "        let s = \"a\nb\"; // src/routes/blog/[slug]/+page.wisp:7",
            "pub async fn like(cx: &mut ::wisp::Cx) -> ::wisp::Result<Option<::wisp::Response>> { Ok(::wisp::rt_traits::Answer::answer(super::like(cx)?)) }",
            "pub async fn render(cx: &mut ::wisp::Cx, __o: &mut ::wisp::Out, __wrap:",
            "page_0::tpl_page_0::render(cx, __o, |__o: &mut ::wisp::Out, cx: &::wisp::Cx, __p: &dyn Fn(&mut ::wisp::Out)| layout_0::tpl_layout_0::render(__o, cx, &|__o: &mut ::wisp::Out| __p(__o))).await",
            "    const A: u8 = 1; // src/routes/+layout.wisp:2",
            "        let p = cx.path(); // src/routes/+layout.wisp:3",
            "pub mod db {",
            "Err(e) => ::wisp::rt::input::failed(cx, e)?,",
        ] {
            assert!(code.contains(want), "{want}\n{code}");
        }
        assert!(!code.contains("pub mod own"), "{code}");
        let err = |name, files: &[(&str, &str)]| app(name, files).unwrap_err();
        assert!(
            err(
                "both",
                &[
                    page("---\nlet a = 1;\n---\n"),
                    ("src/routes/blog/[slug]/+page.rs", "")
                ]
            )
            .contains("and +page.rs is beside it")
        );
        assert!(
            err(
                "load",
                &[page(
                    "---\nstruct Data;\nfn load() -> Data { Data }\nlet a = 1;\n---\n"
                )]
            )
            .starts_with("src/routes/blog/[slug]/+page.wisp:4: the statements of a `---` block")
        );
        assert!(
            err("open", &[page("\n---\nlet a = 1;\n")])
                .starts_with("src/routes/blog/[slug]/+page.wisp:2:1: this `---` starts")
        );
        assert!(
            err(
                "comp",
                &[page("x"), ("src/components/Card.wisp", "---\n---\nx")]
            )
            .contains("a component takes")
        );
    }

    #[test]
    fn hooks_are_checked() {
        let page = ("src/routes/+page.wisp", "x");
        let hooks = |src: &'static str| ("src/hooks.rs", src);
        let code = app("hooks-ok", &[page, hooks("async fn init() -> Result<()> { Ok(()) }\nfn before(cx: &mut Cx) -> Option<Response> { None }\nfn helper() {}")]).unwrap();
        for want in [
            "pub async fn init() -> ::wisp::Result<()> { let () = super::init().await?; Ok(()) }",
            "pub async fn before(cx: &mut ::wisp::Cx) -> ::wisp::Result<Option<::wisp::Response>> { Ok(::wisp::rt_traits::Answer::answer(super::before(cx))) }",
            "hooks::__call::init().await?;",
            "if let Some(r) = hooks::__call::before(cx).await? {",
        ] {
            assert!(code.contains(want), "{want}\n{code}");
        }
        let err = |name, src| app(name, &[page, hooks(src)]).unwrap_err();
        assert!(err("typo", "pub fn befor(cx: &mut Cx) {}").contains("`befor` is not a hook"));
        assert!(err("init-cx", "fn init(cx: &mut Cx) {}").contains("has no `cx`"));
        assert!(
            err("before-input", "fn before(cx: &mut Cx, id: u8) {}").contains("takes only `cx`")
        );
        let inner = "//! Hooks.\n#![allow(dead_code)]\nfn init() {}";
        let code = app("inner-hooks", &[page, hooks(inner)]).unwrap();
        assert!(
            code.contains("//! Hooks.\n#![allow(dead_code)]\n    #[allow(unused_imports)]\n    use super::__mods::*;"),
            "{code}"
        );
        assert!(
            err("returns", "pub fn before(cx: &mut Cx) -> u8 { 1 }")
                .contains("`before` returns `u8`")
        );
        let main = ("src/main.rs", "mod hooks;\nwisp::main!();");
        assert!(
            app("main", &[page, hooks("pub fn init() {}"), main])
                .unwrap_err()
                .starts_with("src/main.rs:1: remove `mod hooks`")
        );
    }

    #[test]
    fn server_files_serve_their_route_and_its_id() {
        let s = |src: &'static str| ("src/routes/notes/+server.rs", src);
        let files = [s("#[derive(Rest)]\nstruct N { t: String }\n\
                 fn before(cx: &mut Cx) {}\nfn delete(id: u64) -> Option<()> { None }\n\
                 fn before_create(cx: &mut Cx, n: &mut N) -> Result { Ok(()) }\n\
                 fn after_update(row: &Row<N>, id: u64) {}")];
        let m = model("rest", &files).unwrap();
        // Each route: its pattern, its file's module and `before`, and the
        // method and shim of each handler.
        let shape: Vec<String> = (m.routes.iter())
            .map(|r| {
                let s = r.server.as_ref().unwrap();
                let hs: Vec<String> = (s.handlers.iter())
                    .map(|h| format!("{} {}", h.op.method, h.shim))
                    .collect();
                format!("{} {} {}: {}", r.pattern, s.module, s.before, hs.join(", "))
            })
            .collect();
        assert_eq!(
            shape,
            [
                "/notes server_0 true: get __rest_list, post __rest_create",
                "/notes/[id=int] server_0 true: delete delete, get __rest_get, put __rest_put, patch __rest_patch"
            ]
        );
        let code = app("rest", &files).unwrap();
        for want in [
            "::wisp::rt::rest::list::<super::N>(cx, &__REST_HOOKS)",
            "use ::wisp::rt_traits::ret::*; let __a0 = ::wisp::rt_traits::FromInput::get(cx, \"id\")?; (&&&Ret::new(super::delete(__a0))).respond()",
            "fn __hook_before_create(cx: &mut ::wisp::Cx, v: &mut super::N) -> ::wisp::Result { super::before_create(cx, v)?; Ok(()) }",
            "let () = super::after_update(row, row.id); Ok(())",
            "Hooks { before_create: Some(__hook_before_create), after_update: Some(__hook_after_update), ..",
        ] {
            assert!(code.contains(want), "{want}: {code}");
        }
        let err = |name, src| app(name, &[s(src)]).unwrap_err();
        assert!(
            err(
                "bad-hook",
                "#[derive(Rest)] struct N { t: String }\nfn before_create(n: N) {}"
            )
            .contains(":2: `before_create` takes `cx` and `note: &mut N`, not `N`")
        );
        assert!(
            err("list-id", "fn list(id: u64) {}").contains(":1: `list` answers GET on the route")
        );
        assert!(
            err("two-gets", "fn get() {}\nfn list() {}")
                .contains(":2: `list` and `get` both answer GET")
        );
        assert!(err("none", "fn helper() {}").contains("+server.rs: defines none of"));
        assert!(
            err("checked", "fn post(#[validate(len = 1..)] n: String) {}")
                .contains("checks an action's input")
        );
        let under_id = (
            "src/routes/n/[id]/+server.rs",
            "#[derive(Rest)] struct A { a: u8 }",
        );
        assert!(
            app("rest-id", &[under_id])
                .unwrap_err()
                .contains("already has an `id`")
        );
    }

    #[test]
    fn saved_tables_load_at_startup() {
        let code = app(
            "tables",
            &[
                ("src/routes/+page.wisp", "x"),
                (
                    "src/routes/+page.rs",
                    "static TODOS: Table<String> = Table::saved(\"todos\");\nconst C: Table<u8> = Table::new();",
                ),
                (
                    "src/db.rs",
                    "pub static USERS: wisp::Table<u8> = Table::saved(\"users\");",
                ),
                (
                    "src/routes/api/+server.rs",
                    "#[derive(Rest)] struct N { t: String }",
                ),
            ],
        )
        .unwrap();
        for want in [
            "pub fn __ready() { super::TODOS.ready(); }",
            "pub fn __ready() { super::USERS.ready(); }",
            "pub fn __ready() { super::N::table().ready(); }",
            "__mods::db::__call::__ready();",
            "page_0::__call::__ready();",
        ] {
            assert!(code.contains(want), "{want}\n{code}");
        }
        assert_eq!(code.matches("::__call::__ready();").count(), 3, "{code}");
        assert!(!code.contains("C.ready()"), "{code}");
    }

    #[test]
    fn action_parameters_are_checked() {
        let page = |src: &'static str| ("src/routes/+page.wisp", src);
        let code = app(
            "rules",
            &[page("---\n#[action]\nfn add(#[validate(len = 1..10, email)] t: String, #[validate(min = 1)] n: Option<u8>) {}\n---\nx")],
        )
        .unwrap();
        // Every input is read and checked before the answer, which lists
        // each that did not pass.
        for want in [
            "let __a0 = ::wisp::rt::input::read(&mut __p, ::wisp::rt_traits::FromInput::get(cx, \"t\"))?; \
             if let Some(__v) = &__a0 { __p.check(\"t\", ::wisp::rt_traits::len(__v, 1..10)); \
             __p.check(\"t\", ::wisp::json::check::email(__v)); }",
            "if let Some(__v) = &__a1 { __p.check(\"n\", ::wisp::json::check::min(__v, (1) as f64)); }",
            "let (Some(__a0), Some(__a1), true) = (__a0, __a1, __p.is_empty()) else { return ::wisp::rt::input::refused(__p); };",
        ] {
            assert!(code.contains(want), "{want}: {code}");
        }
        let err = |name, src| app(name, &[page(src)]).unwrap_err();
        assert!(
            err(
                "rule",
                "---\n#[action]\nfn a(#[validate(size = 1)] t: String) {}\n---\nx"
            )
            .contains("has no `size`")
        );
        assert!(
            err(
                "range",
                "---\n#[action]\nfn a(#[validate(len = 5)] t: String) {}\n---\nx"
            )
            .contains("needs a range")
        );
    }

    #[test]
    fn body_limits_are_checked() {
        let page = ("src/routes/+page.wisp", "x");
        let rs = |src: &'static str| ("src/routes/+page.rs", src);
        let files = [page, rs("const BODY_LIMIT: usize = 8 * wisp::MB;")];
        let m = model("limit-ok", &files).unwrap();
        assert_eq!(m.routes[0].body_limit.as_deref(), Some("page_0"));
        let code = app("limit-ok", &files).unwrap();
        assert_eq!(
            facts(&code, "body_limit"),
            ["Some(page_0::__call::BODY_LIMIT)"]
        );
        assert_eq!(facts(&code, "uploads"), ["None"]);
        assert!(
            code.contains("pub const BODY_LIMIT: usize = super::BODY_LIMIT;"),
            "{code}"
        );
        // The page's limit is the page's: the `/[id]` its `+server.rs` also
        // serves has none (its module has no `BODY_LIMIT` to name); the
        // file's own limit is both routes'.
        let m = model(
            "limit-page-only",
            &[
                page,
                rs("const BODY_LIMIT: usize = 1;"),
                ("src/routes/+server.rs", "fn put(id: u64) {}"),
            ],
        )
        .unwrap();
        let limits: Vec<Option<&str>> = m.routes.iter().map(|r| r.body_limit.as_deref()).collect();
        assert_eq!(limits, [Some("page_0"), None]);
        let m = model(
            "limit-server",
            &[(
                "src/routes/a/+server.rs",
                "const BODY_LIMIT: usize = 1;\nfn post() {}\nfn put(id: u64) {}",
            )],
        )
        .unwrap();
        let limits: Vec<Option<&str>> = m.routes.iter().map(|r| r.body_limit.as_deref()).collect();
        assert_eq!(limits, [Some("server_0"), Some("server_0")]);
        assert!(
            app("limit-type", &[page, rs("pub const BODY_LIMIT: u64 = 1;")])
                .unwrap_err()
                .contains("make it a `usize`")
        );
        let layout = [
            ("src/routes/+layout.wisp", "{@render children()}"),
            ("src/routes/+layout.rs", "pub const BODY_LIMIT: usize = 1;"),
            page,
        ];
        assert!(
            app("limit-layout", &layout)
                .unwrap_err()
                .contains("a layout's `BODY_LIMIT` does nothing")
        );
    }

    #[test]
    fn uploads_raise_the_body_limit() {
        let page = |src: &'static str| [("src/routes/+page.wisp", src)];
        let code = app(
            "uploads",
            &page(
                "---\nconst BODY_LIMIT: usize = 64 * KB;\n#[action]\nfn a(#[validate(max_size = 1 * MB)] pic: Image, #[validate(max_size = 500)] mut more: Option<wisp::Image>) {}\n#[action]\nfn b(pic: Image) {}\n---\nx",
            ),
        )
        .unwrap();
        assert_eq!(
            facts(&code, "body_limit"),
            ["Some(page_0::__call::BODY_LIMIT)"]
        );
        assert_eq!(facts(&code, "uploads"), ["Some(page_0::__call::UPLOADS)"]);
        for want in [
            "pub const UPLOADS: usize = 0 + (1 * MB) as usize + (500) as usize;",
            "if let Some(__v) = &__a0 { __p.check(\"pic\", ::wisp::rt_traits::max_size(__v, (1 * MB) as usize)); }",
            "use super::*;",
        ] {
            assert!(code.contains(want), "{want}: {code}");
        }
        let alone = page("---\n#[action]\nfn a(#[validate(max_size = 9)] pic: Image) {}\n---\nx");
        let m = model("uploads-alone", &alone).unwrap();
        let r = &m.routes[0];
        assert_eq!(
            (r.uploads.as_deref(), r.body_limit.as_deref()),
            (Some("page_0"), None)
        );
        let none = model(
            "uploads-none",
            &page("---\n#[action]\nfn a(pic: String) {}\n---\nx"),
        );
        assert!(none.unwrap().routes[0].uploads.is_none());
        let wrong = app(
            "uploads-text",
            &page("---\n#[action]\nfn a(#[validate(max_size = 9)] pic: String) {}\n---\nx"),
        )
        .unwrap_err();
        assert!(
            wrong.contains("+page.wisp:3: `max_size` is for an upload, and `pic` is a `String`"),
            "{wrong}"
        );
    }

    #[test]
    fn constant_pages_are_baked() {
        let files = [
            (
                "src/app.html",
                "<html><head>%wisp.head%</head><body>%wisp.body%</body></html>",
            ),
            (
                "src/routes/+layout.wisp",
                "<nav>{\"a&b\"}</nav>{@render children()}",
            ),
            // A GET never has what an action refused: the form is baked too.
            (
                "src/routes/+page.wisp",
                "<title>T</title><h1>{1}</h1><form action=\"?/add\"><input name=\"x\"></form>",
            ),
            ("src/routes/+page.rs", "#[action]\nfn add() {}"),
            ("src/routes/user/[name]/+page.wisp", "<h1>{name}</h1>"),
            (
                "src/routes/loads/+page.wisp",
                "---\nlet n = 1;\n---\n<h1>n</h1>",
            ),
        ];
        let code = app("baked", &files).unwrap();
        let doc = "<html><head><script defer src=\\\"/_app/wisp.js?v=VERSION\\\"></script><title>T</title></head><body><nav>a&amp;b</nav><h1>1</h1><form action=\\\"?/add\\\" method=\\\"post\\\"><input name=\\\"x\\\"></form></body></html>"
            .replace("VERSION", crate::runtime_version());
        let etag = format!(
            "\\\"{:016x}\\\"",
            fnv1a(doc.replace("\\\"", "\"").as_bytes())
        );
        for want in [
            format!(
                "static BAKED_0: ::wisp::rt::Baked = ::wisp::rt::Baked::new(\"HTTP/1.1 200 OK\\r\\ncontent-type: text/html; charset=utf-8\\r\\netag: {etag}\\r\\ncontent-length: {}\\r\\n\", \"{doc}\", \"{etag}\"); // /",
                doc.replace("\\\"", "\"").len()
            ),
            "(0, Get | Head) => { ::wisp::rt::browser_ok(cx)?; if ::wisp::rt::baked(cx, __o, &BAKED_0) { Ok(()) } else { serve_page_0(cx, __o).await } },".into(),
            // The action's post renders as before.
            "(0, Post) => {".into(),
        ] {
            assert!(code.contains(&want), "{want}\n{code}");
        }
        assert_eq!(code.matches("static BAKED_").count(), 1, "{code}");
    }

    #[test]
    fn cache_keeps_gets() {
        let page = (
            "src/routes/+page.wisp",
            "---\nconst CACHE: u32 = 60;\nlet n = 1;\n---\n{n}",
        );
        let cache = |m: &Model| -> Vec<Option<(String, bool, bool)>> {
            (m.routes.iter())
                .map(|r| (r.cache.as_ref()).map(|c| (c.module.clone(), c.public, r.by_accept())))
                .collect()
        };
        let m = model("cache-ok", &[page]).unwrap();
        assert_eq!(cache(&m), [Some(("page_0".into(), false, false))]);
        let code = app("cache-ok", &[page]).unwrap();
        for want in [
            "pub const CACHE: u32 = super::CACHE;",
            "(0, Get | Head) => { ::wisp::rt::browser_ok(cx)?; { if ::wisp::rt::cached::<false>(cx, __o, false) { return Ok(()); } \
             serve_page_0(cx, __o).await?; ::wisp::rt::keep::<Self, false>(cx, __o, page_0::__call::CACHE, false); Ok(()) } },",
        ] {
            assert!(code.contains(want), "{want}\n{code}");
        }
        let server = (
            "src/routes/api/+server.rs",
            "const CACHE_PUBLIC: u32 = 5;\nfn get() -> u8 { 1 }\nfn post() {}",
        );
        let m = model("cache-server", &[server]).unwrap();
        assert_eq!(cache(&m), [Some(("server_0".into(), true, false))]);
        let code = app("cache-server", &[server]).unwrap();
        for want in [
            "pub const CACHE: u32 = super::CACHE_PUBLIC;",
            "(0, Get | Head) => { ::wisp::rt::endpoint(cx); if ::wisp::rt::cached::<false>(cx, __o, true) { return Ok(()); } \
             ::wisp::rt::respond(__o, server_0::__call::get(cx).await?); ::wisp::rt::keep::<Self, false>(cx, __o, server_0::__call::CACHE, true); Ok(()) }",
            "(0, Post) => { ::wisp::rt::endpoint(cx); if ::wisp::rt::idempotent(cx, __o) { return Ok(()); } \
             server_0::__call::post(cx).await?; ::wisp::rt::no_content(__o); Ok(()) }",
            // The same, sync, with no future: what `handle_now` answers.
            "now: true, sync: ::wisp::Method::Get.bit() | ::wisp::Method::Head.bit() | ::wisp::Method::Post.bit(),",
            "(0, Get | Head) => { ::wisp::rt::hooked(cx); ::wisp::rt::endpoint(cx); \
             if ::wisp::rt::cached::<false>(cx, __o, true) { return Ok(true); } \
             ::wisp::rt::respond(__o, server_0::__call::get_now(cx)?); \
             ::wisp::rt::keep::<Self, false>(cx, __o, server_0::__call::CACHE, true); Ok(true) }",
            "(0, Post) => { ::wisp::rt::hooked(cx); ::wisp::rt::endpoint(cx); \
             if ::wisp::rt::idempotent(cx, __o) { return Ok(true); } \
             server_0::__call::post_now(cx)?; ::wisp::rt::no_content(__o); Ok(true) }",
            "pub fn post_now(cx: &mut ::wisp::Cx) -> ::wisp::Result<()> { super::post(); Ok(()) }",
        ] {
            assert!(code.contains(want), "{want}\n{code}");
        }
        let rs = |src: &'static str| [("src/routes/+page.wisp", "x"), ("src/routes/+page.rs", src)];
        for (name, files, want) in [
            (
                "cache-type",
                rs("const CACHE: u64 = 1;"),
                "make it a `const` `u32`",
            ),
            (
                "cache-static",
                rs("static CACHE: u32 = 1;"),
                "make it a `const` `u32`",
            ),
            (
                "cache-both",
                rs("const CACHE: u32 = 1;\nconst CACHE_PUBLIC: u32 = 1;"),
                "set one of them",
            ),
        ] {
            let err = app(name, &files).unwrap_err();
            assert!(
                err.contains(want) && err.starts_with("src/routes/+page.rs:"),
                "{err}"
            );
        }
        let layout = [
            (
                "src/routes/+layout.wisp",
                "---\nconst CACHE_PUBLIC: u32 = 1;\n---\n{@render children()}",
            ),
            ("src/routes/+page.wisp", "x"),
        ];
        let err = app("cache-layout", &layout).unwrap_err();
        assert!(
            err.contains("a layout's `CACHE_PUBLIC` does nothing"),
            "{err}"
        );
        let twice = [
            (
                "src/routes/+page.wisp",
                "---\nconst CACHE: u32 = 1;\n---\nx",
            ),
            (
                "src/routes/+server.rs",
                "const CACHE: u32 = 1;\nfn post() {}",
            ),
        ];
        let err = app("cache-twice", &twice).unwrap_err();
        assert!(err.contains("set it in one place"), "{err}");

        // A list answers JSON or NDJSON by `accept`: what is kept of its
        // GET is kept apart by it, and only there.
        let rest = [(
            "src/routes/notes/+server.rs",
            "const CACHE: u32 = 5;\n#[derive(Rest)]\nstruct N { t: String }",
        )];
        let m = model("cache-accept", &rest).unwrap();
        assert_eq!(
            cache(&m),
            [
                Some(("server_0".into(), false, true)),
                Some(("server_0".into(), false, false))
            ]
        );
        let code = app("cache-accept", &rest).unwrap();
        for want in [
            "(0, Get | Head) => { ::wisp::rt::endpoint(cx); if ::wisp::rt::cached::<true>(cx, __o, false) { return Ok(()); } \
             ::wisp::rt::respond(__o, server_0::__call::__rest_list(cx).await?); \
             ::wisp::rt::keep::<Self, true>(cx, __o, server_0::__call::CACHE, false); Ok(()) }",
            "(1, Get | Head) => { ::wisp::rt::endpoint(cx); if ::wisp::rt::cached::<false>(cx, __o, false) {",
        ] {
            assert!(code.contains(want), "{want}\n{code}");
        }
        // A list of the file's own answers as it is written: one key.
        let own = [(
            "src/routes/notes/+server.rs",
            "const CACHE: u32 = 5;\n#[derive(Rest)]\nstruct N { t: String }\nfn list() -> Vec<u8> { vec![] }",
        )];
        let m = model("cache-own-list", &own).unwrap();
        assert!(m.routes.iter().all(|r| r.cache.is_some() && !r.by_accept()));
    }

    #[test]
    fn routes_without_parameters_match_whole() {
        let files = [
            ("src/routes/+page.wisp", "x"),
            ("src/routes/about/+page.wisp", "x"),
            ("src/routes/blog/[slug]/+page.wisp", "{slug}"),
            ("src/routes/blog/new/+page.wisp", "x"),
        ];
        let code = app("router", &files).unwrap();
        for want in [
            "let whole = match path.len() {",
            "1 => (path == \"/\").then_some(0), // /",
            "6 => (path == \"/about\").then_some(1), // /about",
            "9 => (path == \"/blog/new\").then_some(2), // /blog/new",
            "let Some(r) = r.strip_prefix(\"blog/\") else { break 'a0; };",
            "let (p1, None) = ::wisp::rt::seg(r) else { break 'a0; };",
            "return Some((3, [p1, E, E, E, E, E, E, E]));",
        ] {
            assert!(code.contains(want), "{want}\n{code}");
        }
        assert!(!code.contains("::wisp::rt::split"), "{code}");
        let code = app("router-flat", &files[..2]).unwrap();
        assert!(
            code.contains(
                "6 => (path == \"/about\").then_some((1, [\"\"; ::wisp::rt::MAX_PARAMS])), // /about"
            ),
            "{code}"
        );
        // Paths of one length, told apart by a byte, then compared whole.
        let five = [
            ("src/routes/echo/+server.rs", "fn get() {}"),
            ("src/routes/json/+server.rs", "fn get() {}"),
            ("src/routes/user/[id]/+server.rs", "fn get(id: String) {}"),
        ];
        let code = app("router-bytes", &five).unwrap();
        for want in [
            "5 => match path.as_bytes()[1] {",
            "101 => (path == \"/echo\").then_some(0), // /echo",
            "let Some(r) = r.strip_prefix(\"user/\") else { break 'a0; };",
        ] {
            assert!(code.contains(want), "{want}\n{code}");
        }
        let rest = [("src/routes/docs/[...path]/+page.wisp", "{path}")];
        let code = app("router-rest", &rest).unwrap();
        assert!(
            code.contains("let mut segs = [\"\"; ::wisp::rt::MAX_SEGS];"),
            "{code}"
        );
    }

    #[test]
    fn release_builds_write_runs_of_text_at_once() {
        let page = (
            "src/routes/+page.wisp",
            "<p title={\"a\"}>{\"<b>\"} {2}</p>{#if on}{x}!{/if}<i>{3}</i>",
        );
        let code = build("runs", &[page], true).unwrap();
        for want in [
            "__o.body.push_str(\"<p title=\\\"a\\\">&lt;b&gt; 2</p>\");",
            "(&::wisp::rt::Text(&(x))).put(&mut __o.body);",
            "__o.body.push_str(\"!\");",
            "__o.body.push_str(\"<i>3</i>\");",
        ] {
            assert!(code.contains(want), "{want}\n{code}");
        }
        // A dev build keeps each text apart, for `wisp dev` to swap.
        let code = build("runs-dev", &[page], false).unwrap();
        assert!(code.contains("__o.body.push_str(__wisp_s(0));"), "{code}");
        assert!(
            code.contains("(&::wisp::rt::Text(&(\"<b>\"))).put(&mut __o.body);"),
            "{code}"
        );
    }

    #[test]
    fn actions_may_answer_with_a_response() {
        let page = ("src/routes/+page.wisp", "x");
        let rs = (
            "src/routes/+page.rs",
            "#[action] fn csv() -> Response { todo!() }\n#[action] fn maybe() -> Result<Option<Response>> { todo!() }",
        );
        let code = app("respond", &[page, rs]).unwrap();
        for want in [
            "{ Ok(::wisp::rt_traits::Answer::answer(super::csv())) }",
            "{ Ok(::wisp::rt_traits::Answer::answer(super::maybe()?)) }",
            "\"csv\" => match page_0::__call::csv(cx).await { Ok(Some(r)) => { ::wisp::rt::respond(__o, r); return Ok(()); } Ok(None) => {} Err(e) => ::wisp::rt::input::failed(cx, e)?, },",
        ] {
            assert!(code.contains(want), "{want}\n{code}");
        }
        let bad = ("src/routes/+page.rs", "#[action] pub fn n() -> u8 { 1 }");
        assert!(
            app("respond-bad", &[page, bad])
                .unwrap_err()
                .contains("or a `Response` to send instead of the page")
        );
    }

    #[test]
    fn inputs_are_read_by_name() {
        let files = [
            ("src/routes/[id]/+page.wisp", "x"),
            (
                "src/routes/[id]/+page.rs",
                "struct Data;\nfn load(id: u64, q: Option<&str>) -> Data { Data }\n\
                 #[action] async fn add(cx: &mut Cx, text: &str, tags: Vec<String>, on: bool) {}",
            ),
            (
                "src/routes/api/+server.rs",
                "fn get(n: Option<u8>) -> Vec<u8> { vec![] }\nfn post(name: String) {}\nfn delete() -> Option<Response> { None }\n\
                 fn put(body: Note) {}\nfn patch(body: Option<String>) {}",
            ),
        ];
        let code = app("inputs", &files).unwrap();
        for want in [
            "let __a0 = ::wisp::rt::input::body(cx)?; super::put(__a0); Ok(())",
            "let __a0 = ::wisp::rt_traits::FromInput::get(cx, \"body\")?;",
            "(0, Options) => { ::wisp::rt::respond(__o, ::wisp::rt::options(\"GET, HEAD, POST, DELETE, PUT, PATCH, OPTIONS\")); Ok(()) }",
            "let __a0 = ::wisp::rt::input::read(&mut __p, ::wisp::rt_traits::FromInput::get(cx, \"id\"))?; \
             let __a1: Option<Option<String>> = ::wisp::rt::input::read(&mut __p, ::wisp::rt_traits::FromInput::get(cx, \"q\"))?; \
             let (Some(__a0), Some(__a1), true) = (__a0, __a1, __p.is_empty()) else { return ::wisp::rt::input::refused(__p); }; \
             Ok(Loaded(super::load(__a0, __a1.as_deref())))",
            "let __a1: Option<String> = ::wisp::rt::input::read(&mut __p, ::wisp::rt_traits::FromInput::get(cx, \"text\"))?; \
             let __a2 = ::wisp::rt::input::read(&mut __p, ::wisp::rt_traits::FromInput::get(cx, \"tags\"))?; \
             let __a3 = ::wisp::rt::input::read(&mut __p, ::wisp::rt_traits::FromInput::get(cx, \"on\"))?; \
             let (Some(__a1), Some(__a2), Some(__a3), true) = (__a1, __a2, __a3, __p.is_empty()) else { return ::wisp::rt::input::refused(__p); }; \
             Ok(::wisp::rt_traits::Answer::answer(super::add(cx, &__a1, __a2, __a3).await?))",
            "let __a0 = ::wisp::rt_traits::FromInput::get(cx, \"name\")?; super::post(__a0); Ok(())",
            "(&&&Ret::new(super::get(__a0))).respond()",
            "(&&&Ret::new(super::delete())).respond()",
        ] {
            assert!(code.contains(want), "{want}\n{code}");
        }
        let bad = [
            ("src/routes/+page.wisp", "x"),
            (
                "src/routes/+page.rs",
                "\n#[action] fn a((x, y): (u8, u8)) {}",
            ),
        ];
        let err = app("inputs-bad", &bad).unwrap_err();
        assert!(
            err.starts_with("src/routes/+page.rs:2: `a` takes `(x, y): (u8, u8)`"),
            "{err}"
        );
        let page = |rs: &'static str| [("src/routes/+page.wisp", "x"), ("src/routes/+page.rs", rs)];
        // `//!` docs and `#![…]` first (CRLF read as LF), then what Wisp adds.
        let top = "//! A page.\r\n#![allow(dead_code)]\r\nuse std::fmt;";
        let code = app("inner", &page(top)).unwrap();
        assert!(
            code.contains("//! A page.\n#![allow(dead_code)]\n    #[allow(unused_imports)]"),
            "{code}"
        );
        assert!(code.contains("use std::fmt;"), "{code}");
        // The prelude comes after the file's own import of it (the one
        // rustc counts as used), and CRLF and a BOM are read as LF.
        let long = "\u{feff}use wisp::prelude::*;\r\nfn helper() {}";
        let files = [
            ("src/routes/+page.wisp", "a\r\nb"),
            ("src/routes/+page.rs", long),
        ];
        let code = app("dup-prelude", &files).unwrap();
        assert!(
            code.contains("include!(")
                && code.find("include!(") < code.find("use ::wisp::prelude::*;"),
            "{code}"
        );
        assert!(!code.contains('\r') && code.contains("\"a\\nb\""), "{code}");
        // An `Err(error(..))` from before `error()` returned the `Result`.
        let old = "\n#[action] fn a() -> Result<()> { Err(error(400, \"no\")) }";
        assert!(
            app("old-error", &page(old))
                .unwrap_err()
                .starts_with("src/routes/+page.rs:2: `error()` returns the `Result`")
        );
    }

    #[test]
    fn attributes_may_be_left_out() {
        let page = ("src/routes/+page.wisp", "<a href={link} title={t}>x</a>");
        let rs = (
            "src/routes/+page.rs",
            "pub struct Data { pub link: Option<String>, pub t: u8 }
pub fn load() -> Data { todo!() }",
        );
        let code = app("optional", &[page, rs]).unwrap();
        assert!(
            code.contains("if let Some(__v) = (&::wisp::rt::Attr(&(link))).get() {"),
            "{code}"
        );
        assert!(code.contains("guard_url"), "{code}");
        // One of two literals is written whole, escaped, at build time; a
        // URL that would run script is left to the runtime's guard.
        let page = (
            "src/routes/+page.wisp",
            "<p class={if on { \"a&\" } else { \"b\" }}>x</p><a href={if on { \"javascript:x\" } else { \"/\" }}>y</a>",
        );
        let rs = (
            "src/routes/+page.rs",
            "pub struct Data { pub on: bool }\npub fn load() -> Data { todo!() }",
        );
        let code = app("either", &[page, rs]).unwrap();
        assert!(
            code.contains(r#"if on { __o.body.push_str(" class=\"a&amp;\""); } else { __o.body.push_str(" class=\"b\""); }"#),
            "{code}"
        );
        assert!(
            code.contains("(&::wisp::rt::Attr(&(if on { \"javascript:x\" }"),
            "{code}"
        );
    }

    #[test]
    fn data_fields_are_in_scope() {
        let rs = "pub struct Data {
    /// how many
    pub n: u32,
    pub title: String,
    pub pair: (u8, Option<&'static str>),
    pub rows: Vec<(u8, u8)>,
    #[allow(dead_code)]
    pub cb: fn(u8) -> u8,
    hidden: u8,
}
pub fn load() -> Data { todo!() }";
        let files = [
            (
                "src/routes/+page.wisp",
                "{n} {title} {#each rows as r}{r.0}{/each}",
            ),
            ("src/routes/+page.rs", rs),
        ];
        let code = app("fields", &files).unwrap();
        for want in [
            "let n = data.n;",
            "let title = &data.title;",
            "let pair = data.pair;",
            "let rows = &data.rows;",
            "let cb = &data.cb;",
            // Private fields too: the template is inside the page's module.
            "let hidden = data.hidden;",
            "pub mod page_0 {",
            "use ::wisp::prelude::*;",
            "pub fn render(__o: &mut ::wisp::Out, cx: &::wisp::Cx, __d: &super::__call::Loaded) {",
            "let data: &super::Data = &__d.0;",
            "let d = page_0::__call::load(cx).await?;",
            "page_0::tpl_page_0::render(__o, cx, &d)",
        ] {
            assert!(
                code.contains(want),
                "{want}
{code}"
            );
        }
    }

    /// The browser side of a page (with `load` if `loads`) made of `src`.
    fn page_client(src: &str, loads: bool) -> Result<Client, String> {
        let t = Tpl {
            id: 7,
            module: "tpl_page_0".into(),
            rel: "src/routes/+page.wisp".into(),
            kind: Kind::Page,
            user: Some(("page_0".into(), loads)),
            data: Vec::new(),
            load_js: None,
            stmts: None,
            t: template::parse(src).unwrap(),
        };
        let cx = ClientCx {
            comps: &[],
            templates: &[],
            as_client: false,
            lib_hash: "0",
            load: None,
            release: false,
            extra: "/_app/c/extra.js",
        };
        client(&t, &cx).map(|c| c.expect("the page has browser code"))
    }

    #[test]
    fn modules() {
        let src = "<p>hi</p>\n<button on:click=\"toggle\" :hidden=\"data.done\">x</button>\n\n\n\n\n\n\n<script>\n  import a from 'a'\n  import {\n    b } from \"b\";\n  let open = false\n  function toggle() { open = !open }\n</script>";
        let c = page_client(src, true).unwrap();
        assert_eq!(c.id, "t7");
        let head = format!(
            "import {{ define }} from \"/_app/live.js?v={}\";\nimport a from 'a'\nimport {{\n    b }} from \"b\";\n\
             define(\"t7\", function (__wisp_p, __wisp_h) {{ const {{ {HELPERS} }} = __wisp_h; {{ const {{ data }} = __wisp_props(__wisp_p, [\"data\"]); {{\n",
            crate::runtime_version()
        );
        assert!(c.source.starts_with(&head), "{}", c.source);
        // Blank lines, then the script with its imports blanked out: `let
        // open` is on line 13 of the file, and of the module.
        let blanked = format!(
            "\n\n\n\n{}\n{}\n{}\n  let open = __wisp_s(false)\n",
            " ".repeat(19),
            " ".repeat(10),
            " ".repeat(17)
        );
        assert!(c.source[head.len()..].starts_with(&blanked), "{}", c.source);
        assert_eq!(
            c.source
                .lines()
                .position(|l| l == "  let open = __wisp_s(false)"),
            Some(12)
        );
        let tail = "  function toggle() { open.v = !open.v }\nreturn { g: [\n  [[\"on\", \"click\", 8192, (_, event) => toggle(event)], [\"attr\", \"hidden\", () => (data.v.done)]],\n] };\n} } });\n\
                    //# sourceURL=wisp:///src/routes/+page.wisp\n";
        assert!(c.source.ends_with(tail), "{}", c.source);
        assert_eq!(
            c.blob,
            [
                Piece::Text("{\"data\":{\"done\":".into()),
                Piece::Value {
                    expr: "data.done".into(),
                    line: 2
                },
                Piece::Text("}}".into())
            ]
        );
        assert_eq!(c.hash, format!("{:016x}", fnv1a(c.source.as_bytes())));

        // Without `load`, `data` is JavaScript's; without a script, the
        // module only has bindings.
        let c = page_client("<p :text=\"data\"></p>", false).unwrap();
        assert!(
            !c.source.contains("__wisp_props(") && c.source.contains("[[\"text\", () => (data)]]"),
            "{}",
            c.source
        );
        assert_eq!(c.blob, [Piece::Text("{}".into())]);
    }

    #[test]
    fn handlers_and_bindings() {
        let src = "<input on:input.debounce.300ms=\"q = event.target.value\" on:keydown.enter=\"if (ok) send(); else warn()\" \
                   on:click=\"a(); b() // why\" bind:value=\"form.q\" bind:this=\"el\" style:--x=\"x\" transition:fly=\"{ y: 4 }\" \
                   use:tip=\"'hi'\" use:focus class:on=\"{ a: 1 }.a\">";
        let c = page_client(src, false).unwrap();
        let group = c.source.lines().find(|l| l.starts_with("  [[")).unwrap();
        let want = [
            "[\"on\", \"input\", 8192, (_, event) => (q = event.target.value), null, 300]",
            "[\"on\", \"keydown\", 8192, (_, event) => { if (ok) send(); else warn() }, [\"enter\"]]",
            "[\"on\", \"click\", 8192, (_, event) => { a(); b() // why\n }]",
            "[\"bind\", \"value\", () => (form.q), (_, __wisp_v) => { (form.q) = __wisp_v }]",
            // No script declares `el`: the binding does, as state.
            "[\"bind\", \"this\", null, (_, __wisp_v) => { (el.v) = __wisp_v }]",
            "[\"style\", \"--x\", () => (x)]",
            "[\"transition\", \"fly\", () => ({ y: 4 }), 0]",
            "[\"use\", () => tip, () => ('hi')]",
            "[\"use\", () => focus, null]",
            "[\"class\", \"on\", () => ({ a: 1 }.a)]",
        ];
        let got = c
            .source
            .split("return { g: [\n  [")
            .nth(1)
            .unwrap()
            .split("],\n]")
            .next()
            .unwrap();
        assert_eq!(got, want.join(", "), "{group}");
    }

    #[test]
    fn loop_values_are_sent_per_element() {
        let src = "{#each data.keys as key, i}\n{@const n: u8 = 1}\n<b on:click=\"press(key.letter, i, n)\" :title=\"key.mark.label() + data.x.length\"></b>{/each}\
                   {#if let Some((a, b)) = data.pair}<i :text=\"a + b\"></i>{/if}\
                   {#match data.m}{:case Mark::Hit { at: spot, .. } if spot > 1}<u :text=\"spot\"></u>{:case _}{/match}\
                   <template each=\"key in keys\"><s :text=\"key\"></s></template><p :text=\"key\"></p>";
        let c = page_client(src, true).unwrap();
        let text = |ps: &[Piece]| {
            ps.iter()
                .map(|p| match p {
                    Piece::Text(t) => t.clone(),
                    Piece::Value { expr, .. } => format!("<{expr}>"),
                })
                .collect::<String>()
        };
        assert_eq!(
            text(&c.locals[0]),
            "{\"key\":{\"letter\":<key.letter>,\"mark\":<key.mark>},\"i\":<i>,\"n\":<n>}"
        );
        assert_eq!(text(&c.locals[1]), "{\"a\":<a>,\"b\":<b>}");
        assert_eq!(text(&c.locals[2]), "{\"spot\":<spot>}");
        // A client `<template each>` name is the browser's; outside the
        // loop, `key` is JavaScript's too.
        assert!(c.locals[3..].iter().all(Vec::is_empty));
        assert_eq!(text(&c.blob), "{\"data\":{\"x\":<data.x>}}");
        assert!(
            c.source.contains(
                "[\"on\", \"click\", 8192, ({ key, i, n }, event) => (press(key.letter, i, n))]"
            ),
            "{}",
            c.source
        );
        assert!(
            c.source.contains(
                "[\"attr\", \"title\", ({ key }) => (key.mark.label() + data.v.x.length)]"
            ),
            "{}",
            c.source
        );
        assert!(
            c.source.contains("[\"text\", ({ key }) => (key)]"),
            "{}",
            c.source
        );
    }

    #[test]
    fn server_names_the_script_declares_are_refused() {
        let err = page_client("<p>x</p>\n<script>\n  let a = 1, data = 2\n</script>", true)
            .err()
            .unwrap();
        assert_eq!(
            err,
            "src/routes/+page.wisp:3:14: `data` is both a server value and a script variable; rename one"
        );
        let err = page_client(
            "{#each xs as guess}<p :text=\"guess\"></p>{/each}<script>function guess() {}</script>",
            false,
        )
        .err()
        .unwrap();
        assert!(
            err.starts_with("src/routes/+page.wisp:1:65: `guess` is both"),
            "{err}"
        );
        // Unused, a loop name is no one's business.
        assert!(
            page_client(
                "{#each xs as guess}<p :text=\"x\"></p>{/each}<script>let guess</script>",
                false
            )
            .is_ok()
        );
    }

    #[test]
    fn components_send_their_props() {
        let card = (
            "src/components/Card.wisp",
            "{@props title: &str, n: u8 = 0}\n<h2 :text=\"title.toUpperCase()\">{title}</h2><script>const twice = n * 2</script>",
        );
        let page = (
            "src/routes/+page.wisp",
            "<Card title=\"a\" /><Card title=\"b\" />",
        );
        let code = app("live-props", &[card, page]).unwrap();
        // The script reads `n`, then the directive reads `title`.
        assert!(code.contains("__b.push_str(\"{\\\"n\\\":\");\n            ::wisp::rt::json(__b, &(n)); // src/components/Card.wisp:2"), "{code}");
        assert!(code.contains("::wisp::rt::json(__b, &(title)); // src/components/Card.wisp:2\n            __b.push_str(\"}\");"), "{code}");
        assert!(
            code.contains("\"/_app/c/t1.js\" => Some(&tpl_component_0::__WISP_CLIENT),"),
            "{code}"
        );
        assert!(
            code.contains(
                "const { n, title } = __wisp_props(__wisp_p, [\\\"n\\\", \\\"title\\\"]);"
            ),
            "{code}"
        );
    }

    #[test]
    fn props_rune_and_islands() {
        let item = (
            "src/components/Item.wisp",
            "{@props label: &str, count: u32 = 0}\n<button on:click=\"count++\">{:label}</button>\n\
             <script>\n  let { label, count = $bindable(1) } = $props()\n</script>",
        );
        let page = (
            "src/routes/+page.wisp",
            "<Item label=\"a\" client:visible /><Item label=\"b\" client:media=\"(min-width: 800px)\" /><Item label=\"c\" client:load />",
        );
        let code = app("runes-props", &[item, page]).unwrap();
        // Every prop the rune names is sent; its default is the browser's.
        assert!(
            code.contains(
                "const { label, count } = __wisp_props(__wisp_p, [\\\"label\\\", \\\"count\\\"], { count: () => (1) });"
            ),
            "{code}"
        );
        assert!(code.contains("::wisp::rt::live_how(__o, \"v\");"), "{code}");
        assert!(
            code.contains("::wisp::rt::live_how(__o, \"m(min-width: 800px)\");"),
            "{code}"
        );
        assert_eq!(code.matches("live_how").count(), 2, "{code}");

        let err = |files: &[(&str, &str)]| app("runes-bad", files).err().unwrap();
        let bad = err(&[
            ("src/components/Plain.wisp", "<p>hi</p>"),
            ("src/routes/+page.wisp", "<Plain client:idle />"),
        ]);
        assert!(bad.contains("<Plain> has no browser code"), "{bad}");
        // On an element, it and what is inside wait.
        let code = app(
            "runes-el",
            &[(
                "src/routes/+page.wisp",
                "<p client:visible on:click=\"n++\">{:n}</p><script>let n = 0</script>",
            )],
        )
        .unwrap();
        assert!(
            code.contains("[\\\"wait\\\", \\\"v\\\"], [\\\"on\\\""),
            "{code}"
        );
        let bad = err(&[
            ("src/components/Item.wisp", item.1),
            ("src/routes/+page.wisp", "<Item label=\"a\" client:soon />"),
        ]);
        assert!(bad.contains("client:soon"), "{bad}");
        // Once a component names its props, only a $bindable one may be bound.
        let bad = err(&[
            (
                "src/components/Item.wisp",
                "{@props label: &str, count: u32 = 0}\n<b>{:label}</b><script>let { label, count } = $props()</script>",
            ),
            (
                "src/routes/+page.wisp",
                "<Item label={:\"x\"} bind:count=\"n\" /><script>let n = 0</script>",
            ),
        ]);
        assert!(bad.contains("is not bindable"), "{bad}");
        let bad = err(&[
            (
                "src/components/Item.wisp",
                "{@props label: &str}\n<b>{:label}</b><script>let { label, size } = $props()</script>",
            ),
            ("src/routes/+page.wisp", "<Item label=\"x\" />"),
        ]);
        assert!(
            bad.contains("src/components/Item.wisp:2:") && bad.contains("`size` is not a prop"),
            "{bad}"
        );
        let bad = err(&[(
            "src/routes/+page.wisp",
            "<p>{:x}</p><script>let { x } = $props()</script>",
        )]);
        assert!(bad.contains("for components"), "{bad}");
        let bad = err(&[("src/routes/+page.wisp", "<p :text=\"$state(1)\"></p>")]);
        assert!(
            bad.contains("src/routes/+page.wisp:1: `$state` goes in the script"),
            "{bad}"
        );
    }

    #[test]
    fn patterns_bind_names() {
        assert_eq!(pattern_names("(k, v)"), ["k", "v"]);
        assert_eq!(pattern_names("Some(Point { x, y: py, .. })"), ["x", "py"]);
        assert_eq!(pattern_names("std::option::Option::Some(ref mut x)"), ["x"]);
        assert_eq!(pattern_names("n @ 1..=5u8"), ["n"]);
        assert_eq!(
            pattern_names("_ | Status::Draft | \"a\" | 'b'"),
            [] as [&str; 0]
        );
        assert_eq!(guardless("Some(x) if x > 1"), "Some(x)");
        assert_eq!(let_pattern(" Some(u) = data.user"), "Some(u)");
    }

    #[test]
    fn json_trees() {
        let p = |s: &str, line: u32| (s.split('.').map(String::from).collect::<Vec<_>>(), line);
        let tree = json_tree(&[
            p("data.a.b", 1),
            p("data.c", 2),
            p("data.a", 3),
            p("data.c", 4),
            p("data.type", 5),
        ]);
        assert_eq!(
            tree,
            [
                Piece::Text("{\"data\":{\"c\":".into()),
                Piece::Value {
                    expr: "data.c".into(),
                    line: 2
                },
                Piece::Text(",\"a\":".into()),
                Piece::Value {
                    expr: "data.a".into(),
                    line: 3
                },
                Piece::Text(",\"type\":".into()),
                Piece::Value {
                    expr: "data.r#type".into(),
                    line: 5
                },
                Piece::Text("}}".into()),
            ]
        );
        assert_eq!(
            server_path(&["data".into(), "list".into(), "length".into()]),
            ["data", "list"]
        );
        assert_eq!(server_path(&["data".into(), "$x".into()]), ["data"]);
    }

    #[test]
    fn path_encoding() {
        assert_eq!(encode_path("über uns"), "%C3%BCber%20uns");
        assert_eq!(
            encode_path("a-b_c.d~e!$&'()*+,;=:@"),
            "a-b_c.d~e!$&'()*+,;=:@"
        );
    }

    #[test]
    fn imports_are_rewritten() {
        let v = crate::runtime_version();
        let src = "import { store } from 'wisp'\nimport a from '$lib/a.js'\nexport * from '../b.js'\nimport './c.js'\nimport x from 'https://esm.sh/x'\nconst y = import('$lib/y.js')\nconst s = 'wisp'";
        let out = rewrite_specifiers(src, "H", Some("sub"));
        assert_eq!(
            out,
            format!(
                "import {{ store }} from \"/_app/live.js?v={v}\"\nimport a from \"/_app/c/lib/a.js?v=H\"\nexport * from \"/_app/c/lib/b.js?v=H\"\n\
                 import \"/_app/c/lib/sub/c.js?v=H\"\nimport x from 'https://esm.sh/x'\nconst y = import(\"/_app/c/lib/y.js?v=H\")\nconst s = 'wisp'"
            )
        );
        // Outside src/lib, a relative path is left as written.
        assert_eq!(
            rewrite_specifiers("import './c.js'", "H", None),
            "import './c.js'"
        );
    }

    #[test]
    fn client_components_are_checked() {
        let comp = (
            "src/components/Card.wisp",
            "{@props title: &str}\n<p>{title}</p>",
        );
        let page = (
            "src/routes/+page.wisp",
            "{:#each xs as x}<Card title={:x} />{:/each}<script>let xs = []</script>",
        );
        assert!(
            app("client-comp-server", &[comp, page])
                .unwrap_err()
                .contains("its markup has server code")
        );
        let comp = (
            "src/components/Card.wisp",
            "{@props title: &str}\n<p>{:title}</p>",
        );
        let bad = ("src/routes/+page.wisp", "<Card title={:1} nope={:2} />");
        assert!(
            app("client-comp-prop", &[comp, bad])
                .unwrap_err()
                .contains("has no prop `nope`")
        );
        let code = app("client-comp", &[comp, page]).unwrap();
        assert!(
            code.contains(
                "{ html: \\\"<p><template data-w=\\\\\\\"0\\\\\\\"></template><!----></p>\\\" }"
            ),
            "{code}"
        );
    }

    #[test]
    fn components_may_render_themselves() {
        let page = (
            "src/routes/+page.wisp",
            "<Tree node={:{ name: 'r', kids: [] }} />",
        );
        let forever = (
            "src/components/Tree.wisp",
            "{@props node: &str}\n<p>{:node.name}</p><Tree node={:node} />",
        );
        assert!(
            app("tree-forever", &[forever, page])
                .unwrap_err()
                .contains("nothing to stop it")
        );
        let tree = (
            "src/components/Tree.wisp",
            "{@props node: &str}\n<p>{:node.name}</p>{:#each node.kids as kid}<Tree node={:kid} />{/each}",
        );
        let code = app("tree", &[tree, page]).unwrap();
        // It paints itself, a level deeper each time, from its props' JSON.
        for want in [
            "pub fn paint(__o: &mut ::wisp::Out, __p: &[::wisp::rt::Js<'_>]",
            r#"__p[0].get("name").text(&mut __o.body);"#,
            r#"for (__wisp_k0, __wisp_e0) in __p[0].get("kids").items().enumerate() {"#,
            "super::tpl_component_0::paint(__o, &[__wisp_e0], &|__o: &mut ::wisp::Out| {",
            "}, __wisp_d + 1);",
            r#"super::tpl_component_0::paint(__o, &[::wisp::rt::Js("{\"name\":\"r\",\"kids\":[]}")], "#,
        ] {
            assert!(code.contains(want), "{want} in {code}");
        }
        // Two that render each other import each other.
        let a = (
            "src/components/Ping.wisp",
            "{@props n: u8}\n{:#if n}<Pong n={:n} />{/if}",
        );
        let b = (
            "src/components/Pong.wisp",
            "{@props n: u8}\n{:#if n}<Ping n={:n} />{/if}",
        );
        let page = ("src/routes/+page.wisp", "<Ping n={:1} />");
        let code = app("mutual", &[a, b, page]).unwrap();
        assert!(!code.contains("@wisp/comp/"), "{code}");
    }

    #[test]
    fn first_paint_expressions() {
        let known = |path: &[String]| match path[0].as_str() {
            "d" => Pv::Val("D".into()).member(&path[1..]),
            _ => None,
        };
        let paint = |js: &str| match paint_expr(js, &mut known.clone()) {
            Some(Pv::Val(e) | Pv::Num(e) | Pv::Bool(e) | Pv::Test(e)) => e,
            None => "-".into(),
        };
        assert_eq!(paint("d.a.b"), "D.get(\"a\").get(\"b\")");
        assert_eq!(paint("d.list.length"), "D.get(\"list\").length()");
        assert_eq!(paint("!d.x"), "!(D.get(\"x\").truthy())");
        assert_eq!(
            paint("(d.x && !d.y) || d.z.length"),
            "((D.get(\"x\").truthy() && !(D.get(\"y\").truthy())) || D.get(\"z\").length().is_some_and(|n| n > 0))"
        );
        assert_eq!(
            paint("[1, 'a\\'b', { k: -2.50, 'q': [true, null,] }]"),
            "::wisp::rt::Js(\"[1,\\\"a'b\\\",{\\\"k\\\":-2.5,\\\"q\\\":[true,null]}]\")"
        );
        assert_eq!(paint("undefined"), "::wisp::rt::Js(\"null\")");
        for no in [
            "f(d)",
            "d.a()",
            "d[0]",
            "d?.a",
            "d.a + 1",
            "x",
            "d.a === 1",
            "{ k: x }",
            "`t`",
        ] {
            assert_eq!(paint(no), "-", "{no}");
        }
    }
}
