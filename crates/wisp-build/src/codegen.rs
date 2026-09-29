//! Emits `$OUT_DIR/wisp.rs`: user modules, one render function per template,
//! the router `match`, and the `wisp::App` impl.
//!
//! The output is meant to be read. When rustc reports an error inside a
//! template expression, the offending line ends with `// file.wisp:line`.
//!
//! A template with browser code (a client script, directives) also gets an
//! ES module, built here as text and compiled in: see `client`.

use crate::routes::Seg;
use crate::rust_scan::{self, FnItem, Returns};
use crate::template::{self, Code, Dir, Directive, Node, PropDecl, PropValue, Script, Template};
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
    /// User module whose items the template can see, and whether it has `load`.
    user: Option<(String, bool)>,
    /// The `pub` fields of its `Data`, which the template reads by name.
    data: Vec<(String, String)>,
    /// A page's `+page.js`.
    load_js: Option<PathBuf>,
    t: Template,
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
    let read = |p: &Path| fs::read_to_string(p).map_err(|e| format!("{}: {e}", p.display()));
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
    // A `+layout.rs` or `+page.rs`, whose `load` the template reads.
    let scan = |p: &Path| -> Result<rust_scan::Items, String> {
        let items = rust_scan::scan(&read(p)?).map_err(|e| format!("{}:{e}", rel(p)))?;
        items.check_load().map_err(|e| format!("{}:{e}", rel(p)))?;
        Ok(items)
    };

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
    let layout_load = |i: usize| layout_fns[i].iter().find(|f| f.name == "load");
    let layout_has_load = |i: usize| layout_load(i).is_some();

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
    // A route's `BODY_LIMIT`, checked: public, a `usize`, set once.
    let body_limit = |info: &mut RouteInfo,
                      items: &rust_scan::Items,
                      file: &Path,
                      module: String|
     -> Result<(), String> {
        let Some(c) = items.constant("BODY_LIMIT") else {
            return Ok(());
        };
        let at = |msg: &str| format!("{}:{}: {msg}", rel(file), c.line);
        if !c.public {
            return Err(at(
                "`BODY_LIMIT` must be `pub` for Wisp to read it: `pub const BODY_LIMIT: usize = ...`",
            ));
        }
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
            if r.page_rs {
                let rs = r.dir.join("+page.rs");
                let items = scan(&rs)?;
                body_limit(&mut info, &items, &rs, format!("page_{i}"))?;
                info.data = items.data_fields();
                info.page_fns = items.fns;
            }
            let has_load = info.page_fns.iter().any(|f| f.name == "load");
            if let Some(f) = info.page_fns.iter().find(|f| f.action && f.name == "load") {
                return Err(format!(
                    "{}:{}: `{}` cannot be both load and an action",
                    rel(&r.dir.join("+page.rs")),
                    f.line,
                    f.name
                ));
            }
            for f in info.page_fns.iter().filter(|f| f.action) {
                f.check_public()
                    .map_err(|e| format!("{}:{e}", rel(&r.dir.join("+page.rs"))))?;
            }
            if let Some(f) = info.page_fns.iter().find(|f| f.name == "entries") {
                f.check_public()
                    .map_err(|e| format!("{}:{e}", rel(&r.dir.join("+page.rs"))))?;
                if f.takes_cx || f.is_async || f.action {
                    return Err(format!(
                        "{}:{}: `entries` is `pub fn entries() -> Vec<...>`: no `cx`, not async, not an action",
                        rel(&r.dir.join("+page.rs")),
                        f.line
                    ));
                }
            }
            if let Some(f) = info
                .page_fns
                .iter()
                .find(|f| f.action && f.returns_kind() == Returns::Other)
            {
                return Err(format!(
                    "{}:{}: action `{}` returns `{}`. An action returns nothing (or `Result<()>`, so it can use `?`), \
                     or a `Response` to send instead of the page (or `Option<Response>`, to send one only sometimes).",
                    rel(&r.dir.join("+page.rs")),
                    f.line,
                    f.name,
                    f.returns
                ));
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
            body_limit(&mut info, &items, &file, format!("server_{i}"))?;
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
                f.check_public()
                    .map_err(|e| format!("{}:{e}", rel(&file)))?;
            }
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

    // Modules, each after the components it renders in the browser, since
    // it imports them by their content's hash.
    let as_client: Vec<String> = templates.iter().flat_map(|t| client_uses(&t.t)).collect();
    let mut clients: Vec<Option<Client>> = templates.iter().map(|_| None).collect();
    let mut done = vec![false; templates.len()];
    while done.contains(&false) {
        let mut progress = false;
        for (k, t) in templates.iter().enumerate() {
            let uses = client_uses(&t.t);
            let ready = |ci: Option<usize>| ci.is_none_or(|ci| done[ci]);
            if done[k]
                || !uses
                    .iter()
                    .all(|n| ready(comps.iter().position(|c| c.name == *n)))
            {
                continue;
            }
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
            let urls: Vec<Option<String>> = (0..comps.len())
                .map(|ci| clients[ci].as_ref().map(Client::url))
                .collect();
            let is_client = t.kind == Kind::Component && as_client.contains(&comps[k].name);
            let cx = ClientCx {
                comps: &comps,
                templates: &templates,
                urls: &urls,
                as_client: is_client,
                lib_hash: &lib_hash,
                load,
            };
            clients[k] = client(t, &cx)?;
            done[k] = true;
            progress = true;
        }
        if !progress {
            let k = done.iter().position(|d| !d).expect("one is left");
            return Err(format!(
                "{}: components the browser renders cannot use each other in a circle (this one ends up rendering itself)",
                templates[k].rel
            ));
        }
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

    // User modules, declared with #[path] so rust-analyzer treats them as
    // ordinary modules of the crate.
    if hooks.is_some() {
        g.module_decl("hooks", &root.join("src").join("hooks.rs"));
    } else {
        g.line(0, "pub mod hooks {}");
    }
    for (i, l) in tree.layouts.iter().enumerate() {
        if l.has_rs {
            g.module_decl(&format!("layout_{i}"), &l.dir.join("+layout.rs"));
        }
    }
    for (i, r) in tree.routes.iter().enumerate() {
        if r.page_rs {
            g.module_decl(&format!("page_{i}"), &r.dir.join("+page.rs"));
        }
        if r.server {
            g.module_decl(&format!("server_{i}"), &r.dir.join("+server.rs"));
        }
    }
    g.line(0, "");

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
        g.template(t, &comps, c.as_ref());
    }

    // Page handlers: run loads outermost-first, then render inside the layouts.
    for (i, r) in tree.routes.iter().enumerate() {
        let Some(ti) = infos[i].page_tpl else {
            continue;
        };
        let page = &templates[ti];
        g.line(0, "#[allow(unused_variables)]");
        g.line(0, &format!("async fn serve_page_{i}(cx: &mut ::wisp::Cx, __o: &mut ::wisp::Out) -> ::wisp::Result<()> {{"));
        for &l in &r.layouts {
            if let Some(load) = layout_load(l) {
                g.line(
                    1,
                    &format!("let d{l} = {};", call(&format!("layout_{l}"), load)),
                );
            }
        }
        let page_load = infos[i].page_fns.iter().find(|f| f.name == "load");
        if let Some(load) = page_load {
            g.line(1, &format!("let d = {};", call(&format!("page_{i}"), load)));
        }
        let inner = format!(
            "{}::render(__o{})",
            page.module,
            if page_load.is_some() { ", &d" } else { "" }
        );
        g.line(
            1,
            &format!("{};", wrap_layouts(&r.layouts, &layout_has_load, inner)),
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
            if let Some(load) = layout_load(l) {
                g.line(
                    1,
                    &format!("let d{l} = {};", call(&format!("layout_{l}"), load)),
                );
            }
        }
        let inner = format!("tpl_error_{i}::render(__o, status, message)");
        g.line(
            1,
            &format!("{};", wrap_layouts(&e.layouts, &layout_has_load, inner)),
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
        for (k, seg) in exp.iter().enumerate() {
            match seg {
                Seg::Static(s) => pat.push(lit(&encode_path(s))),
                Seg::Param(n) | Seg::Optional(n) => {
                    pat.push(format!("p{k}"));
                    values[names.iter().position(|x| x == n).unwrap()] = format!("*p{k}");
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
                "[{}] => ({id}, [{}]), // {}",
                pat.join(", "),
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
            g.line(3, &format!("{i} => Some({m}::BODY_LIMIT),"));
        }
        g.line(3, "_ => None,");
        g.line(2, "}");
    }
    g.line(1, "}");
    g.line(0, "");

    g.line(1, "async fn init() -> ::wisp::Result<()> {");
    if let Some(init) = hooks
        .as_ref()
        .and_then(|h| h.iter().find(|f| f.name == "init"))
    {
        g.line(2, &format!("let () = {};", call("hooks", init)));
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
                &format!("{} => Some(&{}::__WISP_CLIENT),", lit(&c.path()), t.module),
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
        let entries = match info.page_fns.iter().find(|f| f.name == "entries") {
            Some(_) => format!(
                "Some(|| page_{i}::entries().into_iter().map(::wisp::Entry::params).collect())"
            ),
            None => "None".into(),
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
    if let Some(before) = hooks
        .as_ref()
        .and_then(|h| h.iter().find(|f| f.name == "before"))
    {
        g.line(2, "// src/hooks.rs");
        g.line(2, &answer(before, &call("hooks", before)));
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
                            answer(a, &call(&format!("page_{i}"), a))
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
                    "({i}, {variants}) => {{ ::wisp::rt::respond(__o, {}); Ok(()) }}",
                    call(&format!("server_{i}"), f)
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

/// `module::name(cx)` the way its signature asks: without `cx` if it takes
/// no parameters, awaited if it is async, and with `?` if it returns a Result.
fn call(module: &str, f: &FnItem) -> String {
    let arg = if f.takes_cx { "cx" } else { "" };
    let wait = if f.is_async { ".await" } else { "" };
    let try_ = if f.fallible { "?" } else { "" };
    format!("{module}::{}({arg}){wait}{try_}", f.name)
}

/// The statement that runs `call` of `f` (an action or `before`) in
/// `handle`: a `Response` it returns is sent instead of the page. `let ()`
/// makes any other value a compile error rather than a dropped one.
fn answer(f: &FnItem, call: &str) -> String {
    match f.returns_kind() {
        Returns::Response => format!("::wisp::rt::respond(__o, {call}); return Ok(());"),
        Returns::MaybeResponse => {
            format!("if let Some(r) = {call} {{ ::wisp::rt::respond(__o, r); return Ok(()); }}")
        }
        Returns::Nothing | Returns::Other => format!("let () = {call};"),
    }
}

/// The functions of `src/hooks.rs`, if there is one, checked: `init` and
/// `before` are the only public functions, and look the way they are called.
fn hooks(root: &Path) -> Result<Option<Vec<FnItem>>, String> {
    // A `mod hooks;` of the app's own would compile the file a second time,
    // with statics of its own.
    let main = root.join("src").join("main.rs");
    if let Ok(src) = fs::read_to_string(&main) {
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
    let src = fs::read_to_string(&file).map_err(|e| format!("src/hooks.rs: {e}"))?;
    let fns = rust_scan::scan(&src)
        .map_err(|e| format!("src/hooks.rs:{e}"))?
        .fns;
    for f in &fns {
        let at = |msg: String| format!("src/hooks.rs:{}: {msg}", f.line);
        if f.action {
            return Err(at(format!(
                "`{}` is marked #[action], but actions belong in a +page.rs",
                f.name
            )));
        }
        match f.name.as_str() {
            "init" => {
                f.check_public().map_err(|e| format!("src/hooks.rs:{e}"))?;
                if f.takes_cx {
                    return Err(at("`init` runs once, before the server takes requests, so it has no `cx`: `pub async fn init()`".into()));
                }
                if f.returns_kind() != Returns::Nothing {
                    return Err(at(format!(
                        "`init` returns `{}`; it returns nothing, or `Result<()>` so it can use `?`",
                        f.returns
                    )));
                }
            }
            "before" => {
                f.check_public().map_err(|e| format!("src/hooks.rs:{e}"))?;
                if f.returns_kind() == Returns::Other {
                    return Err(at(format!(
                        "`before` returns `{}`. It returns nothing (or `Result<()>`), or a `Response` to send instead of the page \
                         (or `Option<Response>`, to send one only sometimes).",
                        f.returns
                    )));
                }
            }
            name if f.public => {
                return Err(at(format!(
                    "`{name}` is not a hook: src/hooks.rs has `init` and `before`. Make it private if it is a helper."
                )));
            }
            _ => {}
        }
    }
    Ok(Some(fns))
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

/// `tpl_layout_0::render(__o, &d0, &|__o| tpl_layout_3::render(__o, &|__o| inner))`
fn wrap_layouts(layouts: &[usize], has_load: &dyn Fn(usize) -> bool, inner: String) -> String {
    layouts.iter().rev().fold(inner, |acc, &l| {
        let data = if has_load(l) {
            format!(", &d{l}")
        } else {
            String::new()
        };
        format!("tpl_layout_{l}::render(__o{data}, &|__o: &mut ::wisp::Out| {acc})")
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
            Node::Fragment(body) => check_components(body, t, comps, rel, in_head)?,
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
                    let blank = children
                        .iter()
                        .all(|n| matches!(n, Node::Text(i) if t.chunks[*i].trim().is_empty()));
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

    fn module_decl(&mut self, name: &str, file: &Path) {
        self.line(0, &format!("#[path = {}]", lit(&file.to_string_lossy())));
        self.line(0, &format!("pub mod {name};"));
    }

    fn template(&mut self, t: &Tpl, comps: &[Comp], client: Option<&Client>) {
        self.line(0, &format!("// {}", t.rel));
        self.line(0, "#[doc(hidden)]");
        self.line(0, "#[allow(unused_imports, unused_variables, unused_mut, unused_parens, unused_braces, dead_code, clippy::all)]");
        self.line(0, &format!("pub mod {} {{", t.module));
        // How `{expr}` is written: see `wisp::rt::Text`.
        self.line(
            1,
            "use ::wisp::rt::{Always as _, Direct as _, Formatted as _, Maybe as _};",
        );
        if let Some((m, _)) = &t.user {
            self.line(1, &format!("use super::{m}::*;"));
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
            Some((m, true)) => format!(", data: &super::{m}::Data"),
            _ => String::new(),
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
            target: "body",
            each_depth: 0,
            client,
        };
        self.nodes(&t.t.nodes, 2, &mut cx);
        if client.is_some() {
            self.line(2, "::wisp::rt::live_end(__o);");
        }
        self.line(1, "}");
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
            Node::Render => self.line(ind, "children(__o);"),
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
                let call = format!("super::{}::render(__o{args}", c.module);
                match children {
                    Some(body) => {
                        self.line(
                            ind,
                            &format!("{call}, &|__o: &mut ::wisp::Out| {{ // {}:{line}", cx.rel),
                        );
                        self.nodes(body, ind + 1, cx);
                        self.line(ind, "});");
                    }
                    None => self.line(
                        ind,
                        &format!("{call}, &|_: &mut ::wisp::Out| {{}}); // {}:{line}", cx.rel),
                    ),
                }
            }
            Node::Fragment(body) => self.nodes(body, ind, cx),
            Node::Hole { group } => {
                let c = cx.client.expect("a template with directives has a module");
                if let Some(place) = &c.ssr[*group] {
                    let line = cx.template.groups[*group].line;
                    self.line(
                        ind,
                        &format!(
                            "::wisp::rt::js_text(&mut {buf}, &({place})); // {}:{line}",
                            cx.rel
                        ),
                    );
                }
            }
            Node::Live { group } => {
                let c = cx.client.expect("a template with directives has a module");
                let push = |s: String| format!("{buf}.push_str({});", lit(&s));
                if cx.template.groups[*group].nested {
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
}

struct Emit<'a> {
    rel: &'a str,
    template: &'a Template,
    comps: &'a [Comp],
    target: &'static str,
    each_depth: usize,
    client: Option<&'a Client>,
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
    /// Per group: for a `{:path}` the server knows, the Rust place whose
    /// value is its first paint.
    ssr: Vec<Option<String>>,
}

impl Client {
    fn path(&self) -> String {
        format!("/_app/c/{}.js", self.id)
    }

    fn url(&self) -> String {
        format!("{}?v={}", self.path(), self.hash)
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
const HELPERS: &str = "tick, setTimeout, setInterval, requestAnimationFrame, addEventListener, listen, onMount, onDestroy, effect, derived, \
                       store, persisted, emit, setContext, getContext, goto, invalidate, page, navigating, enhance";

/// What `client` needs to know beyond the template.
struct ClientCx<'a> {
    comps: &'a [Comp],
    templates: &'a [Tpl],
    /// Per component: its module's URL, once built.
    urls: &'a [Option<String>],
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
    let mut groups = Vec::new();
    let mut locals = Vec::new();
    let mut ssr = Vec::new();
    let mut imports: Vec<String> = Vec::new();
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
        let mut first_paint = None;
        for d in &g.directives {
            if d.kind == Dir::Comp {
                let (b, url) = comp_binding(d, &mut names, cx).map_err(|m| at(d.line, d.col, m))?;
                imports.extend(url.filter(|u| !imports.contains(u)));
                bindings.push(b);
                continue;
            }
            bindings.push(binding(d, &mut names)?);
            // `{:data.title}`: the server knows it, so it is the first paint.
            if let (Dir::Hole, Some(v)) = (d.kind, &d.value)
                && js::is_path(&v.src)
            {
                let path: Vec<String> = v.src.split('.').map(|s| s.trim().to_string()).collect();
                let known = (scope.contains(&path[0]) && !g.locals.contains(&path[0]))
                    || (server.contains(&path[0]) && !(cx.load.is_some() && path[0] == "data"));
                if known && server_path(&path).len() == path.len() {
                    first_paint = Some(rust_place(&path));
                }
            }
        }
        groups.push(bindings);
        locals.push(if rust.is_empty() {
            Vec::new()
        } else {
            json_tree(&rust)
        });
        ssr.push(first_paint);
    }

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
        script,
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
    Ok(Some(Client {
        id,
        source,
        hash,
        blob,
        locals,
        ssr,
    }))
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
            Node::Fragment(body) => client_html(body, t, out),
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
    script: Option<&'a Script>,
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
    if let Some(sc) = m.script {
        // Imports go first, as a module's must; they leave blank lines.
        let spans = js::imports(&sc.src);
        for &(a, b) in &spans {
            s.push_str(&rewrite_specifiers(&sc.src[a..b], m.lib_hash, None));
            s.push('\n');
        }
        body = js::blank(&sc.src, &spans);
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
    if let Some(sc) = m.script {
        let next = s.matches('\n').count() as u32 + 1;
        for _ in next..sc.line {
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
/// renders it: `["comp", ID, (L) => props, binds, events]`, and the URL of
/// the component's module, which the page's module imports.
fn comp_binding(
    d: &Directive,
    names: &mut Names,
    cx: &ClientCx,
) -> Result<(String, Option<String>), String> {
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
            PropValue::Expr(_) => unreachable!("the parser refuses server props here"),
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
    Ok((b, cx.urls[ci].clone()))
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
                let pat = let_pattern(&c.src);
                let mut colon = None;
                template::for_each_top(pat, |i| {
                    let b = pat.as_bytes();
                    if b[i] == b':'
                        && b.get(i + 1) != Some(&b':')
                        && (i == 0 || b[i - 1] != b':')
                        && colon.is_none()
                    {
                        colon = Some(i);
                    }
                });
                scope.extend(pattern_names(&pat[..colon.unwrap_or(pat.len())]));
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
            Node::Fragment(body) => {
                let k = scope.len();
                rust_scopes(body, scope, out);
                scope.truncate(k);
            }
            _ => {}
        }
    }
    scope.truncate(outer);
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
        let code = app("hooks-ok", &[page, hooks("pub async fn init() -> Result<()> { Ok(()) }\npub fn before(cx: &mut Cx) -> Option<Response> { None }\nfn helper() {}")]).unwrap();
        assert!(code.contains("let () = hooks::init().await?;"), "{code}");
        assert!(
            code.contains("if let Some(r) = hooks::before(cx) {"),
            "{code}"
        );
        let err = |name, src| app(name, &[page, hooks(src)]).unwrap_err();
        assert!(err("typo", "pub fn befor(cx: &mut Cx) {}").contains("`befor` is not a hook"));
        assert!(err("private", "fn before(cx: &mut Cx) {}").contains("must be `pub`"));
        assert!(err("init-cx", "pub fn init(cx: &mut Cx) {}").contains("has no `cx`"));
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
            &[page, rs("pub const BODY_LIMIT: usize = 8 * wisp::MB;")],
        )
        .unwrap();
        assert!(code.contains("0 => Some(page_0::BODY_LIMIT),"), "{code}");
        assert!(
            app("limit-pub", &[page, rs("const BODY_LIMIT: usize = 1;")])
                .unwrap_err()
                .contains("must be `pub`")
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
            "#[action] pub fn csv() -> Response { todo!() }\n#[action] pub fn maybe() -> Result<Option<Response>> { todo!() }",
        );
        let code = app("respond", &[page, rs]).unwrap();
        assert!(
            code.contains("\"csv\" => { ::wisp::rt::respond(__o, page_0::csv()); return Ok(()); }"),
            "{code}"
        );
        assert!(
            code.contains("\"maybe\" => { if let Some(r) = page_0::maybe()? {"),
            "{code}"
        );
        let bad = ("src/routes/+page.rs", "#[action] pub fn n() -> u8 { 1 }");
        assert!(
            app("respond-bad", &[page, bad])
                .unwrap_err()
                .contains("or a `Response` to send instead of the page")
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
        ] {
            assert!(
                code.contains(want),
                "{want}
{code}"
            );
        }
        assert!(!code.contains("let hidden"), "{code}");
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
            urls: &[],
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
}
