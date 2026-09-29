//! Emits `$OUT_DIR/wisp.rs`: user modules, one render function per template,
//! the router `match`, and the `wisp::App` impl.
//!
//! The output is meant to be read. When rustc reports an error inside a
//! template expression, the offending line ends with `// file.wisp:line`.
//!
//! A template with browser code (a client script, directives) also gets an
//! ES module, built here as text and compiled in: see `client`.

use crate::js::Kind as JsKind;
use crate::routes::Seg;
use crate::rust_scan::{self, FnItem, Returns};
use crate::template::{self, Code, Dir, Directive, Node, PropDecl, PropValue, Template};
use crate::{fnv1a, js, shell};
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
    /// The items of `__call`.
    shims: Vec<String>,
}

impl UserMod {
    /// Whether it has a shim for the function `name`.
    fn has(&self, name: &str) -> bool {
        self.shims
            .iter()
            .any(|s| s.starts_with(&format!("pub async fn {name}(")))
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
/// `kind` says, whatever `f` returns. The file's errors are the caller's to
/// prefix with its path.
fn shim(f: &FnItem, kind: Shim) -> Result<String, String> {
    let mut lets = String::new();
    let mut args = Vec::new();
    let mut inputs = f.inputs()?.into_iter();
    for (_, ty) in &f.params {
        if rust_scan::is_cx(ty) {
            args.push("cx".to_string());
            continue;
        }
        let (name, ty) = inputs.next().expect("an input per parameter but cx");
        let v = format!("__a{}", args.len());
        let t: String = ty.chars().filter(|c| !c.is_whitespace()).collect();
        let inner = t.strip_prefix("Option<").and_then(|r| r.strip_suffix('>'));
        let read = |how: &str| format!("::wisp::rt::input::{how}(cx, {})", lit(name));
        let (line, arg) = if is_str_ref(&t) {
            (
                format!("let {v}: String = {}?;", read("required")),
                format!("&{v}"),
            )
        } else if inner.is_some_and(is_str_ref) {
            (
                format!("let {v}: Option<String> = {}?;", read("optional")),
                format!("{v}.as_deref()"),
            )
        } else if t == "bool" {
            (format!("let {v} = {};", read("flag")), v)
        } else {
            let how = match rust_scan::last_segment(&t) {
                "Option" => "optional",
                "Vec" => "all",
                _ => "required",
            };
            (format!("let {v} = {}?;", read(how)), v)
        };
        lets.push_str(&line);
        lets.push(' ');
        args.push(arg);
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
        Shim::Answer => {
            let back = match f.returns_kind() {
                Returns::Response => format!("Ok(Some({call}))"),
                Returns::MaybeResponse => format!("Ok({call})"),
                Returns::Nothing | Returns::Other => format!("let () = {call}; Ok(None)"),
            };
            format!(
                "pub async fn {name}(cx: &mut ::wisp::Cx) -> ::wisp::Result<Option<::wisp::Response>> {{ {lets}{back} }}"
            )
        }
        Shim::Endpoint => {
            let back = match f.returns_kind() {
                Returns::Response => format!("Ok({call})"),
                Returns::MaybeResponse => {
                    format!("{call}.ok_or_else(|| ::wisp::Error::new(404, \"Not Found\"))")
                }
                Returns::Nothing => format!("let () = {call}; Ok(::wisp::Response::empty(204))"),
                Returns::Other => format!("Ok(::wisp::Response::json_of(&{call}))"),
            };
            format!(
                "pub async fn {name}(cx: &mut ::wisp::Cx) -> ::wisp::Result<::wisp::Response> {{ {lets}{back} }}"
            )
        }
        Shim::Init => {
            format!("pub async fn init() -> ::wisp::Result<()> {{ let () = {call}; Ok(()) }}")
        }
    })
}

/// `&str` or `&'a str`, without spaces.
fn is_str_ref(t: &str) -> bool {
    match t.strip_prefix('&').and_then(|r| r.strip_suffix("str")) {
        Some("") => true,
        Some(life) => {
            life.starts_with('\'')
                && life[1..]
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'_')
        }
        None => false,
    }
}

pub fn generate(input: &Input) -> Result<String, String> {
    let root = input.root;
    let routes_dir = root.join("src").join("routes");
    let tree = crate::routes::scan(&routes_dir)?;

    let rel = |p: &Path| -> String {
        p.strip_prefix(root)
            .unwrap_or(p)
            .to_string_lossy()
            .replace('\\', "/")
    };
    let read = |p: &Path| crate::read_source(p).map_err(|e| format!("{}: {e}", p.display()));
    // A page, layout or error page: everything but a component.
    let parse = |p: &Path| -> Result<Template, String> {
        let t = template::parse(&read(p)?).map_err(|e| format!("{}:{e}", rel(p)))?;
        if let Some((_, line)) = t.props {
            return Err(format!(
                "{}:{line}: only components, in src/components, take props",
                rel(p)
            ));
        }
        Ok(t)
    };
    // A `+layout.rs`, `+page.rs` or `+server.rs`.
    let scan = |p: &Path| -> Result<rust_scan::Items, String> {
        let items = rust_scan::scan(&read(p)?).map_err(|e| format!("{}:{e}", rel(p)))?;
        items.check().map_err(|e| format!("{}:{e}", rel(p)))?;
        Ok(items)
    };
    // The modules of the app's own Rust files, and the shims in them.
    let mut user_mods: Vec<UserMod> = Vec::new();

    let mut g = Gen {
        out: String::new(),
        release: input.release,
    };
    let mut templates: Vec<Tpl> = Vec::new();
    let mut tpl_paths: Vec<(String, u64)> = Vec::new();

    // Template 0 is always the document shell.
    let shell_path = root.join("src").join("app.html");
    let shell_src = if shell_path.exists() {
        read(&shell_path)?
    } else {
        shell::DEFAULT.to_string()
    };
    let shell_chunks = shell::split(&shell_src).map_err(|e| format!("src/app.html: {e}"))?;
    tpl_paths.push(("src/app.html".into(), shell::SHAPE));

    // Components.
    let mut comps: Vec<Comp> = Vec::new();
    let comp_dir = root.join("src").join("components");
    if comp_dir.is_dir() {
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
            let name = file_name.trim_end_matches(".wisp").to_string();
            if !template::is_component_name(&name) {
                // `card.wisp` → `Card.wisp`, `UI.wisp` → `Ui.wisp`.
                let clean: String = name
                    .chars()
                    .filter(|c| c.is_ascii_alphanumeric() || *c == '_')
                    .collect();
                let suggest: String = clean
                    .chars()
                    .enumerate()
                    .map(|(i, c)| {
                        if i == 0 {
                            c.to_ascii_uppercase()
                        } else if clean.bytes().any(|b| b.is_ascii_lowercase()) {
                            c
                        } else {
                            c.to_ascii_lowercase()
                        }
                    })
                    .collect();
                return Err(format!(
                    "{}: a component's file name is its tag: a capital letter first, lowercase letters too (a tag in all capitals is HTML), \
                     and only letters, digits and _, such as {suggest}.wisp",
                    rel(&file)
                ));
            }
            if let Some(other) = comps.iter().find(|c| c.name == name) {
                return Err(format!(
                    "{}: there is already a component `{name}` ({})",
                    rel(&file),
                    other.module
                ));
            }
            let t = template::parse(&read(&file)?).map_err(|e| format!("{}:{e}", rel(&file)))?;
            let props = t.props.as_ref().map(|(p, _)| p.clone()).unwrap_or_default();
            let module = format!("tpl_component_{}", comps.len());
            comps.push(Comp {
                name,
                module: module.clone(),
                props,
                children: t.uses_children,
            });
            tpl_paths.push((rel(&file), t.shape));
            templates.push(Tpl {
                id: tpl_paths.len() - 1,
                module,
                rel: rel(&file),
                kind: Kind::Component,
                user: None,
                data: Vec::new(),
                load_js: None,
                t,
            });
        }
    }

    // Layouts.
    let mut layout_fns: Vec<Vec<FnItem>> = Vec::new();
    for (i, l) in tree.layouts.iter().enumerate() {
        let file = l.dir.join("+layout.wisp");
        let t = parse(&file)?;
        if !t.uses_children {
            return Err(format!(
                "{}: a layout must contain {{@render children()}}",
                rel(&file)
            ));
        }
        let items = if l.has_rs {
            scan(&l.dir.join("+layout.rs"))?
        } else {
            rust_scan::Items::default()
        };
        if let Some(a) = items.fns.iter().find(|f| f.action) {
            return Err(format!(
                "{}:{}: layouts cannot have actions (`{}`); put it in a +page.rs",
                rel(&l.dir.join("+layout.rs")),
                a.line,
                a.name
            ));
        }
        if let Some(c) = items.constant("BODY_LIMIT") {
            return Err(format!(
                "{}:{}: a layout's `BODY_LIMIT` does nothing; set it in the +page.rs or +server.rs whose requests it limits",
                rel(&l.dir.join("+layout.rs")),
                c.line
            ));
        }
        let data = items.data_fields();
        let fns = items.fns;
        let has_load = fns.iter().any(|f| f.name == "load");
        let user = l.has_rs.then(|| (format!("layout_{i}"), has_load));
        if l.has_rs {
            let file = l.dir.join("+layout.rs");
            let shims = fns
                .iter()
                .filter(|f| f.name == "load")
                .map(|f| shim(f, Shim::Load))
                .collect::<Result<_, _>>()
                .map_err(|e| format!("{}:{e}", rel(&file)))?;
            user_mods.push(UserMod {
                name: format!("layout_{i}"),
                file,
                shims,
            });
        }
        tpl_paths.push((rel(&file), t.shape));
        templates.push(Tpl {
            id: tpl_paths.len() - 1,
            module: format!("tpl_layout_{i}"),
            rel: rel(&file),
            kind: Kind::Layout,
            user,
            data,
            load_js: None,
            t,
        });
        layout_fns.push(fns);
    }
    let layout_has_load = |i: usize| layout_fns[i].iter().any(|f| f.name == "load");

    // Error pages.
    for (i, e) in tree.errors.iter().enumerate() {
        let file = e.dir.join("+error.wisp");
        let t = parse(&file)?;
        check_no_children(&t, &rel(&file))?;
        tpl_paths.push((rel(&file), t.shape));
        templates.push(Tpl {
            id: tpl_paths.len() - 1,
            module: format!("tpl_error_{i}"),
            rel: rel(&file),
            kind: Kind::Error,
            user: None,
            data: Vec::new(),
            load_js: None,
            t,
        });
    }

    // Pages and endpoints.
    struct RouteInfo {
        page_tpl: Option<usize>,
        page_fns: Vec<FnItem>,
        server_fns: Vec<FnItem>,
        /// The module whose `BODY_LIMIT` applies.
        body_limit: Option<String>,
        data: Vec<(String, String)>,
    }
    // A route's `BODY_LIMIT`, checked: a `usize`, set once.
    let body_limit = |info: &mut RouteInfo,
                      items: &rust_scan::Items,
                      file: &Path,
                      module: String,
                      shims: &mut Vec<String>|
     -> Result<(), String> {
        let Some(c) = items.constant("BODY_LIMIT") else {
            return Ok(());
        };
        let at = |msg: &str| format!("{}:{}: {msg}", rel(file), c.line);
        if c.ty != "usize" {
            return Err(at(&format!(
                "`BODY_LIMIT` is a `{}`; make it a `usize`, such as `20 * wisp::MB`",
                c.ty
            )));
        }
        if info.body_limit.is_some() {
            return Err(at(
                "`BODY_LIMIT` is also set in this route's +page.rs; set it in one place",
            ));
        }
        info.body_limit = Some(module);
        shims.push("pub const BODY_LIMIT: usize = super::BODY_LIMIT;".into());
        Ok(())
    };
    let mut infos = Vec::new();
    for (i, r) in tree.routes.iter().enumerate() {
        let mut info = RouteInfo {
            page_tpl: None,
            page_fns: Vec::new(),
            server_fns: Vec::new(),
            body_limit: None,
            data: Vec::new(),
        };
        if r.page {
            let file = r.dir.join("+page.wisp");
            let t = parse(&file)?;
            check_no_children(&t, &rel(&file))?;
            let rs = r.dir.join("+page.rs");
            let mut shims = Vec::new();
            if r.page_rs {
                let items = scan(&rs)?;
                body_limit(&mut info, &items, &rs, format!("page_{i}"), &mut shims)?;
                info.data = items.data_fields();
                info.page_fns = items.fns;
            }
            let has_load = info.page_fns.iter().any(|f| f.name == "load");
            let at = |f: &FnItem, msg: String| format!("{}:{}: {msg}", rel(&rs), f.line);
            if let Some(f) = info.page_fns.iter().find(|f| f.action && f.name == "load") {
                return Err(at(
                    f,
                    format!("`{}` cannot be both load and an action", f.name),
                ));
            }
            if let Some(f) = info.page_fns.iter().find(|f| f.name == "entries") {
                if !f.params.is_empty() || f.is_async || f.action {
                    return Err(at(
                        f,
                        "`entries` is `fn entries() -> Vec<...>`: no parameters, not async, not an action".into(),
                    ));
                }
                shims.push("pub fn entries() -> Vec<Vec<String>> { super::entries().into_iter().map(::wisp::Entry::params).collect() }".into());
            }
            if let Some(f) = info
                .page_fns
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
            for f in &info.page_fns {
                let kind = match f.name.as_str() {
                    _ if f.action => Shim::Answer,
                    "load" => Shim::Load,
                    _ => continue,
                };
                shims.push(shim(f, kind).map_err(|e| format!("{}:{e}", rel(&rs)))?);
            }
            if r.page_rs {
                user_mods.push(UserMod {
                    name: format!("page_{i}"),
                    file: rs.clone(),
                    shims,
                });
            }
            let user = r.page_rs.then(|| (format!("page_{i}"), has_load));
            tpl_paths.push((rel(&file), t.shape));
            templates.push(Tpl {
                id: tpl_paths.len() - 1,
                module: format!("tpl_page_{i}"),
                rel: rel(&file),
                kind: Kind::Page,
                user,
                data: info.data.clone(),
                load_js: r.page_js.then(|| r.dir.join("+page.js")),
                t,
            });
            info.page_tpl = Some(templates.len() - 1);
        }
        if r.server {
            let file = r.dir.join("+server.rs");
            let items = scan(&file)?;
            let mut shims = Vec::new();
            body_limit(&mut info, &items, &file, format!("server_{i}"), &mut shims)?;
            let fns = items.fns;
            let at = |f: &FnItem, msg: String| format!("{}:{}: {msg}", rel(&file), f.line);
            if let Some(f) = fns.iter().find(|f| f.action) {
                return Err(at(
                    f,
                    format!(
                        "`{}` is marked #[action], but actions belong in a +page.rs",
                        f.name
                    ),
                ));
            }
            if let Some(f) = fns.iter().find(|f| f.name == "head" || f.name == "options") {
                return Err(at(
                    f,
                    format!(
                        "`{}` is never called: HEAD is answered by `get`, and OPTIONS by Wisp",
                        f.name
                    ),
                ));
            }
            info.server_fns = fns
                .into_iter()
                .filter(|f| METHODS.iter().any(|(m, _, _)| *m == f.name))
                .collect();
            if info.server_fns.is_empty() {
                return Err(format!(
                    "{}: defines none of get, post, put, patch, delete",
                    rel(&file)
                ));
            }
            for f in &info.server_fns {
                shims.push(shim(f, Shim::Endpoint).map_err(|e| format!("{}:{e}", rel(&file)))?);
            }
            user_mods.push(UserMod {
                name: format!("server_{i}"),
                file: file.clone(),
                shims,
            });
            let has_actions = info.page_fns.iter().any(|f| f.action);
            for m in info.server_fns.iter().map(|f| f.name.as_str()) {
                if r.page && (m == "get" || (m == "post" && has_actions)) {
                    return Err(format!(
                        "{}: `{m}` conflicts with the page in the same directory",
                        rel(&file)
                    ));
                }
            }
        }
        infos.push(info);
    }

    let hooks = hooks(root)?;
    let has_hook = |name: &str| hooks.as_ref().is_some_and(|h| h.has(name));
    for (m, file) in &tree.matchers {
        let Some(file) = file else { continue };
        let items = rust_scan::scan(&read(file)?).map_err(|e| format!("{}:{e}", rel(file)))?;
        items
            .check_inner()
            .map_err(|e| format!("{}:{e}", rel(file)))?;
        if items.function("matches").is_none() {
            return Err(format!(
                "{}: a param matcher is `fn matches(s: &str) -> bool`, which this file does not have",
                rel(file)
            ));
        }
        user_mods.push(UserMod {
            name: format!("param_{m}"),
            file: file.clone(),
            shims: vec!["pub fn matches(s: &str) -> bool { super::matches(s) }".into()],
        });
    }
    for t in &templates {
        check_components(&t.t.nodes, &t.t, &comps, &t.rel, false)?;
    }

    // Browser JavaScript: `src/lib/**/*.js`, imported as `$lib/…` and
    // served under one hash (so every importer names a file by the same
    // URL), and each page's `+page.js`.
    let mut lib = Vec::new();
    let lib_dir = root.join("src").join("lib");
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
        lib_src.push((path, read(f)?));
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
    let mut js_files: Vec<JsFile> = lib_src
        .iter()
        .map(|(p, src)| {
            let dir = p.rfind('/').map_or("", |i| &p[..i]);
            JsFile {
                path: format!("/_app/c/lib/{p}"),
                hash: lib_hash.clone(),
                source: rewrite_specifiers(src, &lib_hash, Some(dir)),
            }
        })
        .collect();

    // Browser modules.
    let as_client: Vec<String> = templates.iter().flat_map(|t| client_uses(&t.t)).collect();
    let mut clients: Vec<Option<Client>> = Vec::with_capacity(templates.len());
    for (k, t) in templates.iter().enumerate() {
        let load = match &t.load_js {
            Some(f) => {
                let source = rewrite_specifiers(&read(f)?, &lib_hash, None);
                let hash = format!("{:016x}", fnv1a(source.as_bytes()));
                let path = format!("/_app/c/t{}.load.js", t.id);
                let url = format!("{path}?v={hash}");
                js_files.push(JsFile { path, hash, source });
                Some(url)
            }
            None => None,
        };
        let is_client = t.kind == Kind::Component && as_client.contains(&comps[k].name);
        let cx = ClientCx {
            comps: &comps,
            templates: &templates,
            as_client: is_client,
            lib_hash: &lib_hash,
            load,
        };
        clients.push(client(t, &cx)?);
    }
    // A module imports the modules of the components it renders by URLs
    // whose hash covers every module it can reach, so a change in any of
    // them changes the URL. Components may render each other (or
    // themselves) in a circle: a hash over the set needs no order.
    let finals: Vec<String> = (0..clients.len())
        .map(|k| {
            let mut reach = vec![k];
            let mut stack: Vec<usize> = clients[k].as_ref().map_or(Vec::new(), |c| c.uses.clone());
            while let Some(ci) = stack.pop() {
                if !reach.contains(&ci) {
                    reach.push(ci);
                    stack.extend(clients[ci].iter().flat_map(|c| &c.uses));
                }
            }
            reach.sort_unstable();
            let all: String = reach
                .iter()
                .filter_map(|&j| clients[j].as_ref().map(|c| c.hash.as_str()))
                .collect();
            format!("{:016x}", fnv1a(all.as_bytes()))
        })
        .collect();
    for (k, c) in clients.iter_mut().enumerate() {
        let Some(c) = c else { continue };
        for &ci in &c.uses {
            let url = format!("/_app/c/t{}.js?v={}", templates[ci].id, finals[ci]);
            c.source = c
                .source
                .replacen(&js_str(&comp_placeholder(ci)), &js_str(&url), 1);
        }
        c.hash.clone_from(&finals[k]);
    }

    // ---- emit -------------------------------------------------------------

    g.line(
        0,
        &format!(
            "// @generated by wisp-build for {}. Do not edit.",
            root.display()
        ),
    );
    g.line(0, "");
    // Browser modules import the runtime by wisp-build's version.
    g.line(0, &format!(
        "const _: () = assert!(::wisp::rt::same_version({}), \"wisp and wisp-build are different versions: use the same version of both\");",
        lit(env!("CARGO_PKG_VERSION"))
    ));
    g.line(0, "");

    // The app's own Rust files, each included into a module with the
    // prelude, its shims, and the template it gives data to.
    if hooks.is_none() {
        g.line(0, "pub mod hooks {}");
    }
    for m in hooks.iter().chain(&user_mods) {
        g.line(0, &format!("pub mod {} {{", m.name));
        // The file's own `//!` docs and `#![…]` attributes come first, which
        // is only possible with the file written into the module; a file
        // without any is included, so that errors point at it.
        let src = crate::read_source(&m.file).map_err(|e| format!("{}: {e}", rel(&m.file)))?;
        let top = rust_scan::inner_end(&src);
        if top > 0 {
            g.out.push_str(&src[..top]);
            g.out.push('\n');
        }
        // Not twice: a file that imports the prelude itself would be told
        // that one of the two is unused.
        let squeezed: String = src.split_whitespace().collect();
        if !squeezed.contains("usewisp::prelude::*;")
            && !squeezed.contains("use::wisp::prelude::*;")
        {
            g.line(1, "#[allow(unused_imports)]");
            g.line(1, "use ::wisp::prelude::*;");
        }
        if top > 0 {
            g.out.push_str(&src[top..]);
            g.out.push('\n');
        } else {
            g.line(1, &format!("include!({});", lit(&m.file.to_string_lossy())));
        }
        g.line(1, "#[doc(hidden)]");
        g.line(1, "#[allow(unused_variables, clippy::all)]");
        g.line(1, "pub mod __call {");
        for s in &m.shims {
            g.line(2, s);
        }
        g.line(1, "}");
        for (t, c) in templates.iter().zip(&clients) {
            if t.user.as_ref().is_some_and(|(u, _)| *u == m.name) {
                g.template(t, &comps, c.as_ref());
            }
        }
        g.line(0, "}");
        g.line(0, "");
    }

    // Shell.
    g.line(0, "#[doc(hidden)]");
    g.line(0, "pub mod tpl_shell {");
    g.line(
        1,
        &format!(
            "pub static S: [&str; 3] = [{}, {}, {}];",
            lit(&shell_chunks[0]),
            lit(&shell_chunks[1]),
            lit(&shell_chunks[2])
        ),
    );
    g.line(0, "}");
    g.line(0, "");

    for (i, f) in js_files.iter().enumerate() {
        g.line(0, &format!(
            "static __WISP_JS_{i}: ::wisp::ClientModule = ::wisp::ClientModule {{ id: {}, path: {}, url: {}, etag: {}, source: {} }};",
            lit(&f.path),
            lit(&f.path),
            lit(&format!("{}?v={}", f.path, f.hash)),
            lit(&format!("\"{}\"", f.hash)),
            lit(&f.source)
        ));
    }
    for (t, c) in templates.iter().zip(&clients) {
        if t.user.is_none() {
            g.template(t, &comps, c.as_ref());
        }
    }
    let layout_paths: Vec<String> = templates
        .iter()
        .filter(|t| t.kind == Kind::Layout)
        .map(Tpl::path)
        .collect();

    // Page handlers: run loads outermost-first, then render inside the layouts.
    for (i, r) in tree.routes.iter().enumerate() {
        let Some(ti) = infos[i].page_tpl else {
            continue;
        };
        let page = &templates[ti];
        g.line(0, "#[allow(unused_variables)]");
        g.line(0, &format!("async fn serve_page_{i}(cx: &mut ::wisp::Cx, __o: &mut ::wisp::Out) -> ::wisp::Result<()> {{"));
        for &l in &r.layouts {
            if layout_has_load(l) {
                g.line(
                    1,
                    &format!("let d{l} = layout_{l}::__call::load(cx).await?;"),
                );
            }
        }
        let page_load = infos[i].page_fns.iter().any(|f| f.name == "load");
        if page_load {
            g.line(1, &format!("let d = page_{i}::__call::load(cx).await?;"));
        }
        // A `+page.js` load gets the route and its parameters.
        if page.load_js.is_some() {
            let params: Vec<String> = r
                .params()
                .iter()
                .map(|p| format!("({0}, cx.param({0}))", lit(p)))
                .collect();
            g.line(
                1,
                &format!(
                    "::wisp::rt::live_route(__o, {}, &[{}]);",
                    lit(&r.pattern()),
                    params.join(", ")
                ),
            );
        }
        let inner = format!(
            "{}::render(__o{})",
            page.path(),
            if page_load { ", &d" } else { "" }
        );
        g.line(
            1,
            &format!(
                "{};",
                wrap_layouts(&r.layouts, &layout_paths, &layout_has_load, inner)
            ),
        );
        g.line(1, "Ok(())");
        g.line(0, "}");
        g.line(0, "");
    }

    // Error renderers, one per +error.wisp, inside the layouts of its directory.
    for (i, e) in tree.errors.iter().enumerate() {
        g.line(0, "#[allow(unused_variables)]");
        g.line(0, &format!("async fn serve_error_{i}(cx: &mut ::wisp::Cx, __o: &mut ::wisp::Out, status: u16, message: &str) -> ::wisp::Result<()> {{"));
        for &l in &e.layouts {
            if layout_has_load(l) {
                g.line(
                    1,
                    &format!("let d{l} = layout_{l}::__call::load(cx).await?;"),
                );
            }
        }
        let inner = format!("tpl_error_{i}::render(__o, status, message)");
        g.line(
            1,
            &format!(
                "{};",
                wrap_layouts(&e.layouts, &layout_paths, &layout_has_load, inner)
            ),
        );
        g.line(1, "Ok(())");
        g.line(0, "}");
        g.line(0, "");
    }

    // Static assets and CSS.
    let css = css_source(root)?;
    let mut assets: Vec<(String, PathBuf, String)> = Vec::new(); // url, file, etag
    let css_hash = match &css {
        Some(p) => Some(format!(
            "{:016x}",
            fnv1a(&fs::read(p).map_err(|e| format!("{}: {e}", p.display()))?)
        )),
        None => None,
    };
    if input.release {
        if let (Some(p), Some(h)) = (&css, &css_hash) {
            assets.push(("/_app/app.css".into(), p.clone(), h.clone()));
        }
        let static_dir = root.join("static");
        if static_dir.is_dir() {
            let mut files = Vec::new();
            list_files(&static_dir, &mut files)?;
            files.sort();
            for f in files {
                let url = format!(
                    "/{}",
                    encode_path(
                        &f.strip_prefix(&static_dir)
                            .unwrap()
                            .to_string_lossy()
                            .replace('\\', "/")
                    )
                );
                let bytes = fs::read(&f).map_err(|e| format!("{}: {e}", f.display()))?;
                assets.push((url, f, format!("{:016x}", fnv1a(&bytes))));
            }
        }
        for (i, (_, file, etag)) in assets.iter().enumerate() {
            let ext = file
                .extension()
                .map(|e| e.to_string_lossy().to_ascii_lowercase())
                .unwrap_or_default();
            g.line(0, &format!(
                "static ASSET_{i}: ::wisp::Asset = ::wisp::Asset {{ body: include_bytes!({}), ext: {}, etag: {} }};",
                lit(&file.to_string_lossy()),
                lit(&ext),
                lit(&format!("\"{etag}\""))
            ));
        }
        g.line(0, "");
    }

    // The App impl.
    g.line(0, "pub struct App;");
    g.line(0, "");
    g.line(
        0,
        "#[allow(unused_imports, unused_variables, unreachable_patterns)]",
    );
    g.line(0, "impl ::wisp::App for App {");
    g.line(
        1,
        &format!(
            "const ROOT: &'static str = {};",
            lit(&root.to_string_lossy())
        ),
    );
    g.line(
        1,
        &format!(
            "const CSS: Option<&'static str> = {};",
            match &css_hash {
                Some(h) if input.release => format!("Some({})", lit(h)),
                Some(_) => "Some(\"dev\")".into(),
                None => "None".into(),
            }
        ),
    );
    let params: Vec<String> = tree
        .routes
        .iter()
        .map(|r| {
            format!(
                "&[{}]",
                r.params()
                    .iter()
                    .map(|p| lit(p))
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        })
        .collect();
    g.line(
        1,
        &format!(
            "const PARAMS: &'static [&'static [&'static str]] = &[{}];",
            params.join(", ")
        ),
    );
    let tpls: Vec<String> = tpl_paths
        .iter()
        .map(|(p, s)| format!("({}, 0x{s:016x})", lit(p)))
        .collect();
    g.line(
        1,
        &format!(
            "const TEMPLATES: &'static [(&'static str, u64)] = &[{}];",
            tpls.join(", ")
        ),
    );
    g.line(0, "");

    // Router: one slice-pattern match, most specific arm first.
    g.line(1, "fn route<'a>(path: &'a str, segs: &[&'a str]) -> Option<(usize, [&'a str; ::wisp::rt::MAX_PARAMS])> {");
    g.line(2, "let _ = path;");
    g.line(2, "const E: &str = \"\";");
    g.line(2, "Some(match segs {");
    for (exp, id) in tree.arms() {
        let r = &tree.routes[id];
        let names = r.params();
        let mut values = vec!["E".to_string(); crate::routes::MAX_PARAMS];
        let mut pat = Vec::new();
        // A matcher is a guard, so a segment it refuses goes on to the next arm.
        let mut guards = Vec::new();
        for (k, seg) in exp.iter().enumerate() {
            match seg {
                Seg::Static(s) => pat.push(lit(&encode_path(s))),
                Seg::Param(n, m) | Seg::Optional(n, m) => {
                    pat.push(format!("p{k}"));
                    values[names.iter().position(|x| x == n).unwrap()] = format!("*p{k}");
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
                }
                Seg::Rest(n) => {
                    pat.push(format!("p{k} @ .."));
                    values[names.iter().position(|x| x == n).unwrap()] =
                        format!("::wisp::rt::rest(path, p{k})");
                }
            }
        }
        g.line(
            3,
            &format!(
                "[{}]{} => ({id}, [{}]), // {}",
                pat.join(", "),
                if guards.is_empty() {
                    String::new()
                } else {
                    format!(" if {}", guards.join(" && "))
                },
                values.join(", "),
                r.pattern()
            ),
        );
    }
    g.line(3, "_ => return None,");
    g.line(2, "})");
    g.line(1, "}");
    g.line(0, "");

    g.line(1, "fn body_limit(route: usize) -> Option<usize> {");
    let limits: Vec<(usize, &String)> = infos
        .iter()
        .enumerate()
        .filter_map(|(i, info)| Some((i, info.body_limit.as_ref()?)))
        .collect();
    if limits.is_empty() {
        g.line(2, "let _ = route;");
        g.line(2, "None");
    } else {
        g.line(2, "match route {");
        for (i, m) in limits {
            g.line(3, &format!("{i} => Some({m}::__call::BODY_LIMIT),"));
        }
        g.line(3, "_ => None,");
        g.line(2, "}");
    }
    g.line(1, "}");
    g.line(0, "");

    g.line(1, "async fn init() -> ::wisp::Result<()> {");
    if has_hook("init") {
        g.line(2, "hooks::__call::init().await?;");
    }
    g.line(2, "Ok(())");
    g.line(1, "}");
    g.line(0, "");

    g.line(1, "fn shell() -> [&'static str; 3] {");
    if input.release {
        g.line(2, "tpl_shell::S");
    } else {
        g.line(
            2,
            "[0, 1, 2].map(|i| ::wisp::rt::chunk(0, i, tpl_shell::S[i]))",
        );
    }
    g.line(1, "}");
    g.line(0, "");

    g.line(
        1,
        "fn asset(path: &str) -> Option<&'static ::wisp::Asset> {",
    );
    if assets.is_empty() {
        g.line(2, "let _ = path;");
        g.line(2, "None");
    } else {
        g.line(2, "match path {");
        for (i, (url, _, _)) in assets.iter().enumerate() {
            g.line(3, &format!("{} => Some(&ASSET_{i}),", lit(url)));
        }
        g.line(3, "_ => None,");
        g.line(2, "}");
    }
    g.line(1, "}");
    g.line(0, "");

    g.line(
        1,
        "fn client_module(path: &str) -> Option<&'static ::wisp::ClientModule> {",
    );
    let modules: Vec<(&Tpl, &Client)> = templates
        .iter()
        .zip(&clients)
        .filter_map(|(t, c)| Some((t, c.as_ref()?)))
        .collect();
    if modules.is_empty() && js_files.is_empty() {
        g.line(2, "let _ = path;");
        g.line(2, "None");
    } else {
        g.line(2, "match path {");
        for (t, c) in modules {
            g.line(
                3,
                &format!("{} => Some(&{}::__WISP_CLIENT),", lit(&c.path()), t.path()),
            );
        }
        for (i, f) in js_files.iter().enumerate() {
            g.line(3, &format!("{} => Some(&__WISP_JS_{i}),", lit(&f.path)));
        }
        g.line(3, "_ => None,");
        g.line(2, "}");
    }
    g.line(1, "}");
    g.line(0, "");

    // Routes for `wisp build --static`.
    g.line(1, "fn export_routes() -> Vec<::wisp::ExportRoute> {");
    g.line(2, "vec![");
    for (i, r) in tree.routes.iter().enumerate() {
        let info = &infos[i];
        let actions = info.page_fns.iter().any(|f| f.action);
        let entries = match info.page_fns.iter().any(|f| f.name == "entries") {
            true => format!("Some(page_{i}::__call::entries)"),
            false => "None".into(),
        };
        g.line(3, &format!(
            "::wisp::ExportRoute {{ pattern: {}, page: {}, actions: {actions}, server: {}, entries: {entries} }},",
            lit(&r.pattern()),
            r.page,
            r.server
        ));
    }
    g.line(2, "]");
    g.line(1, "}");
    g.line(0, "");

    g.line(1, "async fn handle(route: Option<usize>, cx: &mut ::wisp::Cx, __o: &mut ::wisp::Out) -> ::wisp::Result<()> {");
    g.line(2, "use ::wisp::Method::*;");
    if has_hook("before") {
        g.line(2, "// src/hooks.rs");
        g.line(2, &answer("hooks::__call::before"));
    }
    g.line(2, "::wisp::rt::hooked(cx);");
    g.line(
        2,
        "let Some(route) = route else { return Err(::wisp::Error::new(404, \"Not Found\")) };",
    );
    // live.js asks for the route's error page this way when browser code
    // fails while starting (see `boundary` in live.js).
    g.line(2, "if cx.method == Get && cx.header(\"x-wisp-error\").is_some() { return Err(::wisp::Error::new(500, \"Something went wrong in the browser\")) }");
    g.line(2, "match (route, cx.method) {");
    for (i, r) in tree.routes.iter().enumerate() {
        let info = &infos[i];
        let mut allow: Vec<&str> = Vec::new();
        g.line(3, &format!("// {}", r.pattern()));
        if r.page {
            g.line(
                3,
                &format!("({i}, Get | Head) => serve_page_{i}(cx, __o).await,"),
            );
            allow.extend(["GET", "HEAD"]);
            let actions: Vec<&FnItem> = info.page_fns.iter().filter(|f| f.action).collect();
            if !actions.is_empty() {
                g.line(3, &format!("({i}, Post) => {{"));
                g.line(4, "::wisp::rt::check_origin(cx)?;");
                g.line(4, "match cx.action() {");
                for a in &actions {
                    g.line(
                        5,
                        &format!(
                            "{} => {{ {} }}",
                            lit(&a.name),
                            answer(&format!("page_{i}::__call::{}", a.name))
                        ),
                    );
                }
                g.line(5, "other => return Err(::wisp::rt::no_action(other)),");
                g.line(4, "}");
                g.line(4, &format!("serve_page_{i}(cx, __o).await"));
                g.line(3, "}");
                allow.push("POST");
            }
        }
        for f in &info.server_fns {
            let (_, variants, allowed) = METHODS.iter().find(|(n, _, _)| *n == f.name).unwrap();
            g.line(
                3,
                &format!(
                    "({i}, {variants}) => {{ ::wisp::rt::respond(__o, server_{i}::__call::{}(cx).await?); Ok(()) }}",
                    f.name
                ),
            );
            allow.push(allowed);
        }
        g.line(
            3,
            &format!(
                "({i}, _) => Err(::wisp::rt::method_not_allowed({})),",
                lit(&allow.join(", "))
            ),
        );
    }
    g.line(3, "_ => Err(::wisp::Error::new(404, \"Not Found\")),");
    g.line(2, "}");
    g.line(1, "}");
    g.line(0, "");

    // Nearest error page per route; unmatched URLs use the root one.
    let root_error = tree.errors.iter().position(|e| e.dir == routes_dir);
    g.line(1, "async fn error(route: Option<usize>, cx: &mut ::wisp::Cx, __o: &mut ::wisp::Out, status: u16, message: &str) -> ::wisp::Result<()> {");
    if tree.errors.is_empty() {
        g.line(2, "let _ = route;");
        g.line(2, "::wisp::rt::default_error(cx, __o, status, message);");
        g.line(2, "Ok(())");
    } else {
        let per_route: Vec<String> = tree.routes.iter().map(|r| opt(r.error)).collect();
        g.line(
            2,
            &format!(
                "const BY_ROUTE: [Option<usize>; {}] = [{}];",
                per_route.len(),
                per_route.join(", ")
            ),
        );
        g.line(
            2,
            &format!(
                "let page = match route {{ Some(r) => BY_ROUTE[r], None => {} }};",
                opt(root_error)
            ),
        );
        g.line(2, "match page {");
        for i in 0..tree.errors.len() {
            g.line(
                3,
                &format!("Some({i}) => serve_error_{i}(cx, __o, status, message).await,"),
            );
        }
        g.line(
            3,
            "_ => { ::wisp::rt::default_error(cx, __o, status, message); Ok(()) }",
        );
        g.line(2, "}");
    }
    g.line(1, "}");
    g.line(0, "}");

    Ok(g.out)
}

/// `+server.rs` function, the `Method` variants it serves, its `Allow` entry.
const METHODS: [(&str, &str, &str); 5] = [
    ("get", "Get | Head", "GET, HEAD"),
    ("post", "Post", "POST"),
    ("put", "Put", "PUT"),
    ("patch", "Patch", "PATCH"),
    ("delete", "Delete", "DELETE"),
];

/// The statement in `handle` that runs an `Answer` shim (an action or
/// `before`): a `Response` it hands back is sent instead of the page.
fn answer(shim: &str) -> String {
    format!("if let Some(r) = {shim}(cx).await? {{ ::wisp::rt::respond(__o, r); return Ok(()); }}")
}

/// `src/hooks.rs`, if there is one, checked, as a module with shims for
/// the hooks it has: `init` and `before`, which look the way they are
/// called, and no other public function (a typo would never run).
fn hooks(root: &Path) -> Result<Option<UserMod>, String> {
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
        return Ok(None);
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
                if f.returns_kind() == Returns::Other {
                    return Err(at(format!(
                        "`before` returns `{}`. It returns nothing (or `Result<()>`), or a `Response` to send instead of the page \
                         (or `Option<Response>`, to send one only sometimes).",
                        f.returns
                    )));
                }
                if f.params.iter().any(|(_, ty)| !rust_scan::is_cx(ty)) {
                    return Err(at("`before` runs for every route, so it takes only `cx`: `fn before(cx: &mut Cx)`".into()));
                }
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
    Ok(Some(UserMod {
        name: "hooks".into(),
        file,
        shims,
    }))
}

/// A field type that is certainly `Copy`: a number, `bool`, `char`, a shared
/// reference, or an `Option` or tuple of those. Others are borrowed.
fn is_copy(ty: &str) -> bool {
    let t = ty.trim();
    if let Some(inner) = t.strip_prefix("Option<").and_then(|r| r.strip_suffix('>')) {
        return is_copy(inner);
    }
    if let Some(inner) = t.strip_prefix('(').and_then(|r| r.strip_suffix(')')) {
        return inner.split(',').all(|p| p.trim().is_empty() || is_copy(p));
    }
    (t.starts_with('&') && !t.starts_with("&mut"))
        || matches!(
            t,
            "bool"
                | "char"
                | "u8"
                | "u16"
                | "u32"
                | "u64"
                | "u128"
                | "usize"
                | "i8"
                | "i16"
                | "i32"
                | "i64"
                | "i128"
                | "isize"
                | "f32"
                | "f64"
        )
}

fn opt(x: Option<usize>) -> String {
    x.map_or("None".into(), |i| format!("Some({i})"))
}

/// `layout_0::tpl_layout_0::render(__o, &d0, &|__o| tpl_layout_3::render(__o, &|__o| inner))`,
/// where `paths` has each layout template's module path.
fn wrap_layouts(
    layouts: &[usize],
    paths: &[String],
    has_load: &dyn Fn(usize) -> bool,
    inner: String,
) -> String {
    layouts.iter().rev().fold(inner, |acc, &l| {
        let data = if has_load(l) {
            format!(", &d{l}")
        } else {
            String::new()
        };
        format!(
            "{}::render(__o{data}, &|__o: &mut ::wisp::Out| {acc})",
            paths[l]
        )
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
                    let Some(d) = c.props.iter().find(|d| d.name == p.name) else {
                        return Err(at(format!(
                            "<{name}> has no prop `{}`; it takes {}",
                            p.name,
                            takes()
                        )));
                    };
                    if p.value == PropValue::Flag && d.ty != "bool" {
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
        let text = fs::read_to_string(&src).map_err(|e| format!("{}: {e}", src.display()))?;
        if text.contains("tailwindcss") {
            println!(
                "cargo::warning=src/app.css uses Tailwind but .wisp/app.css is missing; run `wisp dev` or `wisp build`"
            );
        }
        return Ok(Some(src));
    }
    Ok(None)
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
        && b.iter()
            .all(|&c| c.is_ascii_alphanumeric() || c == b'_' || c == b'.')
}

fn borrow_place(expr: &str) -> String {
    if is_field_path(expr) {
        format!("&({expr})")
    } else {
        expr.to_string()
    }
}

/// `let PAT = EXPR` with EXPR borrowed when it is a field path.
fn if_condition(cond: &str) -> String {
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
            borrow_place(rest[i + 1..].trim())
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
        if t.user.is_some() {
            self.line(1, "use super::*;");
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
                "pub static __WISP_CLIENT: ::wisp::ClientModule = ::wisp::ClientModule {{ id: {}, path: {}, url: {}, etag: {}, source: {} }};",
                lit(&c.id),
                lit(&path),
                lit(&url),
                lit(&format!("\"{}\"", c.hash)),
                lit(&c.source)
            ));
        }
        let data = match &t.user {
            Some((_, true)) => ", __d: &super::__call::Loaded",
            _ => "",
        };
        let sig = match t.kind {
            Kind::Page => format!("pub fn render(__o: &mut ::wisp::Out{data})"),
            Kind::Layout => format!(
                "pub fn render(__o: &mut ::wisp::Out{data}, children: &dyn Fn(&mut ::wisp::Out))"
            ),
            Kind::Error => {
                "pub fn render(__o: &mut ::wisp::Out, status: u16, message: &str)".into()
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
        if matches!(t.user, Some((_, true))) {
            self.line(2, "let data: &super::Data = &__d.0;");
            for (name, ty) in &t.data {
                if !matches!(name.as_str(), "data" | "children") {
                    let by = if is_copy(ty) { "" } else { "&" };
                    self.line(2, &format!("let {name} = {by}data.{name};"));
                }
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
            let mut blob = c.blob.clone();
            if let Some(Piece::Text(last)) = blob.last_mut() {
                last.push(']'); // closes the record
            }
            self.json(3, "__b", &blob, &t.rel);
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
        };
        self.nodes(&t.t.nodes, 2, &mut cx);
        if client.is_some() {
            self.line(2, "::wisp::rt::live_end(__o);");
        }
        self.line(1, "}");
        // A component the browser renders: its markup as the browser's copy
        // of it, painted from its props' JSON, for a page to show first.
        // Deep enough, a component rendering itself leaves the rest to the
        // browser.
        if let Some(c) = client.filter(|c| c.paints) {
            self.line(1, "pub fn paint(__o: &mut ::wisp::Out, __p: &[::wisp::rt::Js<'_>], children: &dyn Fn(&mut ::wisp::Out), __wisp_d: u32) {");
            self.line(2, "if __wisp_d > 32 { return; }");
            self.line(2, "__o.body.push_str(\"<!--[-->\");");
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
            self.line(2, "__o.body.push_str(\"<!--]-->\");");
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

    fn nodes(&mut self, nodes: &[Node], ind: usize, cx: &mut Emit) {
        for n in nodes {
            self.node(n, ind, cx);
        }
    }

    fn code_line(&mut self, ind: usize, s: &str, code: &Code, cx: &Emit) {
        self.line(ind, &format!("{s} // {}:{}", cx.rel, code.line));
    }

    fn node(&mut self, n: &Node, ind: usize, cx: &mut Emit) {
        let buf = format!("__o.{}", cx.target);
        match n {
            Node::Text(i) => {
                if self.release {
                    let text = &cx.template.chunks[*i];
                    if !text.is_empty() {
                        self.line(ind, &format!("{buf}.push_str({});", lit(text)));
                    }
                } else {
                    self.line(ind, &format!("{buf}.push_str(__wisp_s({i}));"));
                }
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
            Node::Const(code) => self.code_line(ind, &format!("let {};", code.src), code, cx),
            Node::Render if cx.paint => {
                self.line(
                    ind,
                    &format!("{buf}.push_str(\"<template data-wslot></template>\");"),
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
                        &format!("{kw} {} {{", if_condition(&cond.src)),
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
                let amp = if is_field_path(&iter.src) { "&" } else { "" };
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
                    &format!("match {} {{", borrow_place(&scrutinee.src)),
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
            } => {
                let c = cx
                    .comps
                    .iter()
                    .find(|c| c.name == *name)
                    .expect("check_components found it");
                // Props in the order the component declares them. A reference
                // type takes a borrow of the expression, so `title={post.title}`
                // passes a `&String` where the prop is a `&str`.
                let mut args = String::new();
                for d in &c.props {
                    let by_ref = d.ty.starts_with('&');
                    let arg = match props.iter().find(|p| p.name == d.name).map(|p| &p.value) {
                        Some(PropValue::Expr(code)) if by_ref => format!("&({})", code.src),
                        Some(PropValue::Expr(code)) => format!("({})", code.src),
                        Some(PropValue::Text(text)) if by_ref || d.ty.starts_with("impl") => {
                            lit(text)
                        }
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
                        for n in body.iter().filter(|n| !is_snippet(n)) {
                            self.node(n, ind + 1, cx);
                        }
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
            Node::Client(branches) => self.client_block(branches, ind, cx),
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
            Node::Live { group } => {
                let c = cx.client.expect("a template with directives has a module");
                let push = |s: String| format!("{buf}.push_str({});", lit(&s));
                if cx.template.groups[*group].nested || cx.paint {
                    self.line(ind, &push(format!(" data-w=\"{group}\"")));
                } else {
                    self.line(ind, &push(" data-w=\"".into()));
                    self.line(
                        ind,
                        &format!("(&::wisp::rt::Text(&__wisp_i)).put(&mut {buf});"),
                    );
                    self.line(ind, &push(format!(".{group}\"")));
                }
                // The loop values its directives read, as JSON.
                let locals = &c.locals[*group];
                if !locals.is_empty() {
                    self.line(ind, &push(" data-wl=\"".into()));
                    self.line(ind, "{");
                    self.line(ind + 1, "let __j = &mut ::std::string::String::new();");
                    self.json(ind + 1, "__j", locals, cx.rel);
                    self.line(ind + 1, &format!("::wisp::rt::escape(&mut {buf}, __j);"));
                    self.line(ind, "}");
                    self.line(ind, &push("\"".into()));
                }
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
            push("<!--[-->"),
            push("<!--]-->"),
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
        self.line(ind + 1, &push("<!--[-->"));
        self.nodes(body, ind + 1, cx);
        self.line(ind + 1, &push("<!--]-->"));
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
            let json = literal_json(js, &t[*k..=end])?;
            *k = end + 1;
            Some(Pv::Val(format!("::wisp::rt::Js({})", lit(&json))))
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
        format!("/_app/c/{}.js", self.id)
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
const HELPERS: &str = "tick, setTimeout, setInterval, requestAnimationFrame, addEventListener, listen, onMount, onDestroy, effect, watch, \
                       derived, __wisp_d, store, persisted, emit, setContext, getContext, goto, invalidate, page, navigating, enhance";

/// What `client` needs to know beyond the template.
struct ClientCx<'a> {
    comps: &'a [Comp],
    templates: &'a [Tpl],
    /// Some page renders this component in the browser.
    as_client: bool,
    lib_hash: &'a str,
    /// The page's `+page.js`, served at this URL.
    load: Option<String>,
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
    let server: Vec<String> = match t.kind {
        Kind::Page | Kind::Layout if server_load => vec!["data".into()],
        Kind::Component => tt
            .props
            .iter()
            .flat_map(|(ds, _)| ds)
            .map(|d| d.name.clone())
            .collect(),
        _ => Vec::new(),
    };
    let script = tt.script.as_ref();
    let src = script.map_or("", |s| s.src.as_str());
    let declared = js::declarations(src);
    // A place in the script, as a line and column of the file.
    let script_at = |off: usize| -> (u32, u32) {
        let s = script.expect("an offset in the script");
        let before = &s.src[..off];
        let col = match before.rfind('\n') {
            Some(n) => before[n + 1..].chars().count() as u32 + 1,
            None => s.col + before.chars().count() as u32,
        };
        (s.line + before.matches('\n').count() as u32, col)
    };
    // What the module runs: the script, its `$derived` values compiled.
    let runs = js::derived(src).map_err(|(off, msg)| {
        let (line, col) = script_at(off);
        at(line, col, msg)
    })?;
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

    let mut params: Vec<&str> = Vec::new();
    for (path, _) in &used {
        if !params.contains(&path[0].as_str()) {
            params.push(&path[0]);
        }
    }
    // A component the browser renders takes every prop, from the page's
    // code; a `+page.js` hands the page its `data`.
    if cx.as_client {
        for name in &server {
            if !params.contains(&name.as_str()) && !js::is_reserved(name) {
                params.push(name);
            }
        }
    }
    if cx.load.is_some() && !params.contains(&"data") {
        params.push("data");
    }
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
        script: script.map(|s| (runs.as_str(), s.line)),
        groups: &groups,
        imports: &imports,
        load: cx.load.as_deref(),
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
    let lets = declared
        .iter()
        .filter_map(|(n, _)| Some((n.clone(), js::initializer(src, n)?.to_string())))
        .collect();
    Ok(Some(Client {
        id,
        source,
        hash,
        blob,
        locals,
        uses,
        paints: cx.as_client,
        server: known,
        whole: t.kind == Kind::Component,
        declared: declared.into_iter().map(|(n, _)| n).collect(),
        lets,
        scopes,
    }))
}

/// Where a module's import of component `ci`'s module goes, until the
/// URLs are known (see `generate`).
fn comp_placeholder(ci: usize) -> String {
    format!("@wisp/comp/{ci}")
}

/// The markup of a component the browser renders: its text, with every
/// directive element marked by its group alone.
fn client_html(nodes: &[Node], t: &Template, out: &mut String) {
    for n in nodes {
        match n {
            Node::Text(i) => out.push_str(&t.chunks[*i]),
            Node::Live { group } => {
                let _ = write!(out, " data-w=\"{group}\"");
            }
            Node::Client(branches) => {
                for (g, body) in branches {
                    let _ = write!(out, "<template data-w=\"{g}\">");
                    client_html(body, t, out);
                    out.push_str("</template>");
                }
            }
            Node::Render => out.push_str("<template data-wslot></template>"),
            _ => {}
        }
    }
}

/// A module's parts, for `module_source`.
struct Module<'a> {
    id: &'a str,
    rel: &'a str,
    params: &'a [&'a str],
    /// The script, as it runs, and the line of the file it starts on.
    script: Option<(&'a str, u32)>,
    groups: &'a [Vec<String>],
    /// Modules of the components it renders.
    imports: &'a [String],
    load: Option<&'a str>,
    html: Option<&'a str>,
    lib_hash: &'a str,
}

/// The module's text. The script keeps its line numbers, as far as the
/// lines before it allow, so the browser's errors point into the .wisp file.
///
/// The script runs in blocks of its own, inside the helpers and then the
/// server values, so it may reuse a helper's name and a server value may
/// too. Its function returns the binding groups, and for an instance that a
/// morph keeps, `s` to take new server values and `p` to read them back
/// (for `bind:` on a component).
fn module_source(m: &Module) -> String {
    let mut s = String::new();
    let _ = writeln!(
        s,
        "import {{ define }} from \"/_app/live.js?v={}\";",
        env!("CARGO_PKG_VERSION")
    );
    for url in m.imports {
        let _ = writeln!(s, "import {};", js_str(url));
    }
    if let Some(url) = m.load {
        let _ = writeln!(s, "import * as __wisp_u from {};", js_str(url));
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
    let names = m.params.join(", ");
    let _ = write!(
        s,
        "define({}, function (__wisp_p, __wisp_h) {{ const {{ {HELPERS} }} = __wisp_h; {{ ",
        js_str(m.id)
    );
    if !m.params.is_empty() {
        let _ = write!(s, "let {{ {names} }} = __wisp_p; ");
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
    s.push(']');
    if !m.params.is_empty() {
        let _ = write!(
            s,
            ", s: (__wisp_n) => {{ ({{ {names} }} = __wisp_n) }}, p: () => ({{ {names} }})"
        );
    }
    s.push_str(" };\n} } }");
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
        return Some(format!("/_app/live.js?v={}", env!("CARGO_PKG_VERSION")));
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
    Some(format!("/_app/c/lib/{rel}?v={lib_hash}"))
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
            let mods: Vec<String> = d.mods.iter().map(|m| js_str(m)).collect();
            format!(
                "[\"on\", {name}, [{}], {} => {}]",
                mods.join(", "),
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
        Dir::Transition => format!(
            "[\"transition\", {name}, {}]",
            optional(d.value.as_ref(), names)?
        ),
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
        let key = js_str(&p.name);
        let declared = c.props.iter().any(|x| x.name == p.name);
        if !declared && !matches!(p.value, PropValue::On(_)) {
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

/// A JavaScript (and JSON) string literal.
fn js_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            c if c < ' ' => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// The part of a chain read in the browser that the server sends: its Rust
/// field names, without a last `length` (a list or a string arrives whole,
/// and JavaScript knows its length).
fn server_path(path: &[String]) -> Vec<String> {
    let field = |s: &String| {
        s.bytes()
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic() || c == b'_')
            && s.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_')
    };
    let mut p: Vec<String> = path.iter().take_while(|s| field(s)).cloned().collect();
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
        if !(c.is_ascii_alphanumeric() || c == b'_') {
            i += 1;
            continue;
        }
        let start = i;
        while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_') {
            i += 1;
        }
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
        assert_eq!(
            if_condition("let Some(u) = data.user"),
            "let Some(u) = &(data.user)"
        );
        assert_eq!(
            if_condition("let Some(u) = find(x)"),
            "let Some(u) = find(x)"
        );
        assert_eq!(if_condition("let 1..=5 = n"), "let 1..=5 = n");
        assert_eq!(if_condition("a == b"), "a == b");
        assert_eq!(if_condition("letter"), "letter");
    }

    /// Generates the app made of `files` (path, contents), in a scratch
    /// directory: the error if it fails, `Ok` with the code otherwise.
    fn app(name: &str, files: &[(&str, &str)]) -> Result<String, String> {
        let root = std::env::temp_dir().join(format!("wisp-codegen-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        for (path, contents) in files {
            let p = root.join(path);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(p, contents).unwrap();
        }
        let out = generate(&Input {
            root: &root,
            release: false,
        });
        let _ = fs::remove_dir_all(&root);
        out
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
    fn hooks_are_checked() {
        let page = ("src/routes/+page.wisp", "x");
        let hooks = |src: &'static str| ("src/hooks.rs", src);
        let code = app("hooks-ok", &[page, hooks("async fn init() -> Result<()> { Ok(()) }\nfn before(cx: &mut Cx) -> Option<Response> { None }\nfn helper() {}")]).unwrap();
        for want in [
            "pub async fn init() -> ::wisp::Result<()> { let () = super::init().await?; Ok(()) }",
            "pub async fn before(cx: &mut ::wisp::Cx) -> ::wisp::Result<Option<::wisp::Response>> { Ok(super::before(cx)) }",
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
            code.contains("//! Hooks.\n#![allow(dead_code)]\n    #[allow(unused_imports)]\n    use ::wisp::prelude::*;"),
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
    fn body_limits_are_checked() {
        let page = ("src/routes/+page.wisp", "x");
        let rs = |src: &'static str| ("src/routes/+page.rs", src);
        let code = app(
            "limit-ok",
            &[page, rs("const BODY_LIMIT: usize = 8 * wisp::MB;")],
        )
        .unwrap();
        assert!(
            code.contains("0 => Some(page_0::__call::BODY_LIMIT),"),
            "{code}"
        );
        assert!(
            code.contains("pub const BODY_LIMIT: usize = super::BODY_LIMIT;"),
            "{code}"
        );
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
    fn actions_may_answer_with_a_response() {
        let page = ("src/routes/+page.wisp", "x");
        let rs = (
            "src/routes/+page.rs",
            "#[action] fn csv() -> Response { todo!() }\n#[action] fn maybe() -> Result<Option<Response>> { todo!() }",
        );
        let code = app("respond", &[page, rs]).unwrap();
        for want in [
            "{ Ok(Some(super::csv())) }",
            "{ Ok(super::maybe()?) }",
            "\"csv\" => { if let Some(r) = page_0::__call::csv(cx).await? { ::wisp::rt::respond(__o, r); return Ok(()); } }",
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
                "fn get(n: Option<u8>) -> Vec<u8> { vec![] }\nfn post(name: String) {}\nfn delete() -> Option<Response> { None }",
            ),
        ];
        let code = app("inputs", &files).unwrap();
        for want in [
            "let __a0 = ::wisp::rt::input::required(cx, \"id\")?; let __a1: Option<String> = ::wisp::rt::input::optional(cx, \"q\")?; Ok(Loaded(super::load(__a0, __a1.as_deref())))",
            "let __a1: String = ::wisp::rt::input::required(cx, \"text\")?; let __a2 = ::wisp::rt::input::all(cx, \"tags\")?; \
             let __a3 = ::wisp::rt::input::flag(cx, \"on\"); let () = super::add(cx, &__a1, __a2, __a3).await; Ok(None)",
            "Ok(::wisp::Response::json_of(&super::get(__a0)))",
            "let () = super::post(__a0); Ok(::wisp::Response::empty(204))",
            "super::delete().ok_or_else(|| ::wisp::Error::new(404, \"Not Found\"))",
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
        // The prelude is not imported twice, and CRLF and a BOM are read as LF.
        let long = "\u{feff}use wisp::prelude::*;\r\nfn helper() {}";
        let files = [
            ("src/routes/+page.wisp", "a\r\nb"),
            ("src/routes/+page.rs", long),
        ];
        let code = app("dup-prelude", &files).unwrap();
        assert_eq!(code.matches("use ::wisp::prelude::*;").count(), 0, "{code}");
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
            "pub fn render(__o: &mut ::wisp::Out, __d: &super::__call::Loaded) {",
            "let data: &super::Data = &__d.0;",
            "let d = page_0::__call::load(cx).await?;",
            "page_0::tpl_page_0::render(__o, &d)",
        ] {
            assert!(
                code.contains(want),
                "{want}
{code}"
            );
        }
        assert!(is_copy("&'static str") && is_copy("Option<(u8, char)>"));
        assert!(!is_copy("&mut u8") && !is_copy("Vec<u8>") && !is_copy("Option<String>"));
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
            t: template::parse(src).unwrap(),
        };
        let cx = ClientCx {
            comps: &[],
            templates: &[],
            as_client: false,
            lib_hash: "0",
            load: None,
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
             define(\"t7\", function (__wisp_p, __wisp_h) {{ const {{ {HELPERS} }} = __wisp_h; {{ let {{ data }} = __wisp_p; {{\n",
            env!("CARGO_PKG_VERSION")
        );
        assert!(c.source.starts_with(&head), "{}", c.source);
        // Blank lines, then the script with its imports blanked out: `let
        // open` is on line 13 of the file, and of the module.
        let blanked = format!(
            "\n\n\n\n{}\n{}\n{}\n  let open = false\n",
            " ".repeat(19),
            " ".repeat(10),
            " ".repeat(17)
        );
        assert!(c.source[head.len()..].starts_with(&blanked), "{}", c.source);
        assert_eq!(
            c.source.lines().position(|l| l == "  let open = false"),
            Some(12)
        );
        let tail = "  function toggle() { open = !open }\nreturn { g: [\n  [[\"on\", \"click\", [], (_, event) => toggle(event)], [\"attr\", \"hidden\", () => (data.done)]],\n], \
                    s: (__wisp_n) => { ({ data } = __wisp_n) }, p: () => ({ data }) };\n} } });\n\
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
            !c.source.contains("__wisp_p; {") && c.source.contains("[[\"text\", () => (data)]]"),
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
            "[\"on\", \"input\", [\"debounce\", \"300ms\"], (_, event) => (q = event.target.value)]",
            "[\"on\", \"keydown\", [\"enter\"], (_, event) => { if (ok) send(); else warn() }]",
            "[\"on\", \"click\", [], (_, event) => { a(); b() // why\n }]",
            "[\"bind\", \"value\", () => (form.q), (_, __wisp_v) => { (form.q) = __wisp_v }]",
            "[\"bind\", \"this\", null, (_, __wisp_v) => { (el) = __wisp_v }]",
            "[\"style\", \"--x\", () => (x)]",
            "[\"transition\", \"fly\", () => ({ y: 4 })]",
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
                "[\"on\", \"click\", [], ({ key, i, n }, event) => (press(key.letter, i, n))]"
            ),
            "{}",
            c.source
        );
        assert!(
            c.source
                .contains("[\"attr\", \"title\", ({ key }) => (key.mark.label() + data.x.length)]"),
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
        assert!(code.contains("::wisp::rt::json(__b, &(title)); // src/components/Card.wisp:2\n            __b.push_str(\"}]\");"), "{code}");
        assert!(
            code.contains("\"/_app/c/t1.js\" => Some(&tpl_component_0::__WISP_CLIENT),"),
            "{code}"
        );
        assert!(code.contains("let { n, title } = __wisp_p;"), "{code}");
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
        let v = env!("CARGO_PKG_VERSION");
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
