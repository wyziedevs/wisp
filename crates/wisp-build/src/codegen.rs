//! Emits `$OUT_DIR/wisp.rs`: user modules, one render function per template,
//! the router `match`, and the `wisp::App` impl.
//!
//! The output is meant to be read. When rustc reports an error inside a
//! template expression, the offending line ends with `// file.wisp:line`.

use crate::routes::Seg;
use crate::rust_scan::{self, FnItem};
use crate::template::{self, Code, Node, Template};
use crate::{fnv1a, shell};
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
}

struct Tpl {
    id: usize,
    module: String,
    rel: String,
    kind: Kind,
    /// User module whose items the template can see, and whether it has `load`.
    user: Option<(String, bool)>,
    t: Template,
}

pub fn generate(input: &Input) -> Result<String, String> {
    let root = input.root;
    let routes_dir = root.join("src").join("routes");
    let tree = crate::routes::scan(&routes_dir)?;

    let rel = |p: &Path| -> String { p.strip_prefix(root).unwrap_or(p).to_string_lossy().replace('\\', "/") };
    let read = |p: &Path| fs::read_to_string(p).map_err(|e| format!("{}: {e}", p.display()));
    let parse = |p: &Path| -> Result<Template, String> {
        template::parse(&read(p)?).map_err(|e| format!("{}:{e}", rel(p)))
    };

    let mut g = Gen { out: String::new(), release: input.release };
    let mut templates: Vec<Tpl> = Vec::new();
    let mut tpl_paths: Vec<(String, u64)> = Vec::new();

    // Template 0 is always the document shell.
    let shell_path = root.join("src").join("app.html");
    let shell_src = if shell_path.exists() { read(&shell_path)? } else { shell::DEFAULT.to_string() };
    let shell_chunks = shell::split(&shell_src).map_err(|e| format!("src/app.html: {e}"))?;
    tpl_paths.push(("src/app.html".into(), shell::SHAPE));

    // Layouts.
    let mut layout_fns: Vec<Vec<FnItem>> = Vec::new();
    for (i, l) in tree.layouts.iter().enumerate() {
        let file = l.dir.join("+layout.wisp");
        let t = parse(&file)?;
        if !t.uses_children {
            return Err(format!("{}: a layout must contain {{@render children()}}", rel(&file)));
        }
        let fns = if l.has_rs { rust_scan::top_level_fns(&read(&l.dir.join("+layout.rs"))?) } else { Vec::new() };
        if let Some(a) = fns.iter().find(|f| f.action) {
            return Err(format!("{}: layouts cannot have actions (`{}`)", rel(&l.dir.join("+layout.rs")), a.name));
        }
        let has_load = fns.iter().any(|f| f.name == "load");
        let user = l.has_rs.then(|| (format!("layout_{i}"), has_load));
        tpl_paths.push((rel(&file), t.shape));
        templates.push(Tpl { id: tpl_paths.len() - 1, module: format!("tpl_layout_{i}"), rel: rel(&file), kind: Kind::Layout, user, t });
        layout_fns.push(fns);
    }
    let layout_has_load = |i: usize| layout_fns[i].iter().any(|f| f.name == "load");

    // Error pages.
    for (i, e) in tree.errors.iter().enumerate() {
        let file = e.dir.join("+error.wisp");
        let t = parse(&file)?;
        check_no_children(&t, &rel(&file))?;
        tpl_paths.push((rel(&file), t.shape));
        templates.push(Tpl { id: tpl_paths.len() - 1, module: format!("tpl_error_{i}"), rel: rel(&file), kind: Kind::Error, user: None, t });
    }

    // Pages and endpoints.
    struct RouteInfo {
        page_tpl: Option<usize>,
        page_fns: Vec<FnItem>,
        server_fns: Vec<String>,
    }
    let mut infos = Vec::new();
    for (i, r) in tree.routes.iter().enumerate() {
        let mut info = RouteInfo { page_tpl: None, page_fns: Vec::new(), server_fns: Vec::new() };
        if r.page {
            let file = r.dir.join("+page.wisp");
            let t = parse(&file)?;
            check_no_children(&t, &rel(&file))?;
            if r.page_rs {
                info.page_fns = rust_scan::top_level_fns(&read(&r.dir.join("+page.rs"))?);
            }
            let has_load = info.page_fns.iter().any(|f| f.name == "load");
            if let Some(f) = info.page_fns.iter().find(|f| f.action && f.name == "load") {
                return Err(format!("{}: `{}` cannot be both load and an action", rel(&r.dir.join("+page.rs")), f.name));
            }
            let user = r.page_rs.then(|| (format!("page_{i}"), has_load));
            tpl_paths.push((rel(&file), t.shape));
            templates.push(Tpl { id: tpl_paths.len() - 1, module: format!("tpl_page_{i}"), rel: rel(&file), kind: Kind::Page, user, t });
            info.page_tpl = Some(templates.len() - 1);
        }
        if r.server {
            let file = r.dir.join("+server.rs");
            let fns = rust_scan::top_level_fns(&read(&file)?);
            info.server_fns = fns.into_iter().map(|f| f.name).filter(|n| METHODS.iter().any(|(m, _, _)| m == n)).collect();
            if info.server_fns.is_empty() {
                return Err(format!("{}: defines none of get, post, put, patch, delete", rel(&file)));
            }
            let has_actions = info.page_fns.iter().any(|f| f.action);
            for m in &info.server_fns {
                if r.page && (m == "get" || (m == "post" && has_actions)) {
                    return Err(format!("{}: `{m}` conflicts with the page in the same directory", rel(&file)));
                }
            }
        }
        infos.push(info);
    }

    // ---- emit -------------------------------------------------------------

    g.line(0, &format!("// @generated by wisp-build for {}. Do not edit.", root.display()));
    g.line(0, "");

    // User modules, declared with #[path] so rust-analyzer treats them as
    // ordinary modules of the crate.
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
    g.line(1, &format!("pub static S: [&str; 3] = [{}, {}, {}];", lit(&shell_chunks[0]), lit(&shell_chunks[1]), lit(&shell_chunks[2])));
    g.line(0, "}");
    g.line(0, "");

    for t in &templates {
        g.template(t);
    }

    // Page handlers: run loads outermost-first, then render inside the layouts.
    for (i, r) in tree.routes.iter().enumerate() {
        let Some(ti) = infos[i].page_tpl else { continue };
        let page = &templates[ti];
        g.line(0, "#[allow(unused_variables)]");
        g.line(0, &format!("async fn serve_page_{i}(cx: &mut ::wisp::Cx, __o: &mut ::wisp::Out) -> ::wisp::Result<()> {{"));
        for &l in &r.layouts {
            if layout_has_load(l) {
                g.line(1, &format!("let d{l} = layout_{l}::load(cx).await?;"));
            }
        }
        let page_load = page.user.as_ref().is_some_and(|u| u.1);
        if page_load {
            g.line(1, &format!("let d = page_{i}::load(cx).await?;"));
        }
        let inner = format!("{}::render(__o{})", page.module, if page_load { ", &d" } else { "" });
        g.line(1, &format!("{};", wrap_layouts(&r.layouts, &layout_has_load, inner)));
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
                g.line(1, &format!("let d{l} = layout_{l}::load(cx).await?;"));
            }
        }
        let inner = format!("tpl_error_{i}::render(__o, status, message)");
        g.line(1, &format!("{};", wrap_layouts(&e.layouts, &layout_has_load, inner)));
        g.line(1, "Ok(())");
        g.line(0, "}");
        g.line(0, "");
    }

    // Static assets and CSS.
    let css = css_source(root)?;
    let mut assets: Vec<(String, PathBuf, String)> = Vec::new(); // url, file, etag
    let css_hash = match &css {
        Some(p) => Some(format!("{:016x}", fnv1a(&fs::read(p).map_err(|e| format!("{}: {e}", p.display()))?))),
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
                let url = format!("/{}", encode_path(&f.strip_prefix(&static_dir).unwrap().to_string_lossy().replace('\\', "/")));
                let bytes = fs::read(&f).map_err(|e| format!("{}: {e}", f.display()))?;
                assets.push((url, f, format!("{:016x}", fnv1a(&bytes))));
            }
        }
        for (i, (_, file, etag)) in assets.iter().enumerate() {
            let ext = file.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
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
    g.line(0, "#[allow(unused_imports, unused_variables, unreachable_patterns)]");
    g.line(0, "impl ::wisp::App for App {");
    g.line(1, &format!("const ROOT: &'static str = {};", lit(&root.to_string_lossy())));
    g.line(1, &format!("const CSS: Option<&'static str> = {};", match &css_hash {
        Some(h) if input.release => format!("Some({})", lit(h)),
        Some(_) => "Some(\"dev\")".into(),
        None => "None".into(),
    }));
    let params: Vec<String> = tree.routes.iter().map(|r| format!("&[{}]", r.params().iter().map(|p| lit(p)).collect::<Vec<_>>().join(", "))).collect();
    g.line(1, &format!("const PARAMS: &'static [&'static [&'static str]] = &[{}];", params.join(", ")));
    let tpls: Vec<String> = tpl_paths.iter().map(|(p, s)| format!("({}, 0x{s:016x})", lit(p))).collect();
    g.line(1, &format!("const TEMPLATES: &'static [(&'static str, u64)] = &[{}];", tpls.join(", ")));
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
                    values[names.iter().position(|x| x == n).unwrap()] = format!("::wisp::rt::rest(path, p{k})");
                }
            }
        }
        g.line(3, &format!("[{}] => ({id}, [{}]), // {}", pat.join(", "), values.join(", "), r.pattern()));
    }
    g.line(3, "_ => return None,");
    g.line(2, "})");
    g.line(1, "}");
    g.line(0, "");

    g.line(1, "fn shell() -> [&'static str; 3] {");
    if input.release {
        g.line(2, "tpl_shell::S");
    } else {
        g.line(2, "[0, 1, 2].map(|i| ::wisp::rt::chunk(0, i, tpl_shell::S[i]))");
    }
    g.line(1, "}");
    g.line(0, "");

    g.line(1, "fn asset(path: &str) -> Option<&'static ::wisp::Asset> {");
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

    g.line(1, "async fn handle(route: usize, cx: &mut ::wisp::Cx, __o: &mut ::wisp::Out) -> ::wisp::Result<()> {");
    g.line(2, "use ::wisp::Method::*;");
    g.line(2, "match (route, cx.method) {");
    for (i, r) in tree.routes.iter().enumerate() {
        let info = &infos[i];
        let mut allow: Vec<&str> = Vec::new();
        g.line(3, &format!("// {}", r.pattern()));
        if r.page {
            g.line(3, &format!("({i}, Get | Head) => serve_page_{i}(cx, __o).await,"));
            allow.extend(["GET", "HEAD"]);
            let actions: Vec<&FnItem> = info.page_fns.iter().filter(|f| f.action).collect();
            if !actions.is_empty() {
                g.line(3, &format!("({i}, Post) => {{"));
                g.line(4, "::wisp::rt::check_origin(cx)?;");
                g.line(4, "match cx.action() {");
                for a in &actions {
                    g.line(5, &format!("{} => {{ page_{i}::{}(cx).await?; }}", lit(&a.name), a.name));
                }
                g.line(5, "other => return Err(::wisp::rt::no_action(other)),");
                g.line(4, "}");
                g.line(4, &format!("serve_page_{i}(cx, __o).await"));
                g.line(3, "}");
                allow.push("POST");
            }
        }
        for m in &info.server_fns {
            let (_, variants, allowed) = METHODS.iter().find(|(n, _, _)| n == m).unwrap();
            g.line(3, &format!("({i}, {variants}) => {{ let r = server_{i}::{m}(cx).await?; ::wisp::rt::respond(__o, r); Ok(()) }}"));
            allow.push(allowed);
        }
        g.line(3, &format!("({i}, _) => Err(::wisp::rt::method_not_allowed({})),", lit(&allow.join(", "))));
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
        g.line(2, &format!("const BY_ROUTE: [Option<usize>; {}] = [{}];", per_route.len(), per_route.join(", ")));
        g.line(2, &format!("let page = match route {{ Some(r) => BY_ROUTE[r], None => {} }};", opt(root_error)));
        g.line(2, "match page {");
        for i in 0..tree.errors.len() {
            g.line(3, &format!("Some({i}) => serve_error_{i}(cx, __o, status, message).await,"));
        }
        g.line(3, "_ => { ::wisp::rt::default_error(cx, __o, status, message); Ok(()) }");
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

fn opt(x: Option<usize>) -> String {
    x.map_or("None".into(), |i| format!("Some({i})"))
}

/// `tpl_layout_0::render(__o, &d0, &|__o| tpl_layout_3::render(__o, &|__o| inner))`
fn wrap_layouts(layouts: &[usize], has_load: &dyn Fn(usize) -> bool, inner: String) -> String {
    layouts.iter().rev().fold(inner, |acc, &l| {
        let data = if has_load(l) { format!(", &d{l}") } else { String::new() };
        format!("tpl_layout_{l}::render(__o{data}, &|__o: &mut ::wisp::Out| {acc})")
    })
}

fn check_no_children(t: &Template, rel: &str) -> Result<(), String> {
    if t.uses_children { Err(format!("{rel}: only layouts can use {{@render children()}}")) } else { Ok(()) }
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
            println!("cargo::warning=src/app.css uses Tailwind but .wisp/app.css is missing; run `wisp dev` or `wisp build`");
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
        if b <= b' ' || b >= 0x7f || matches!(b, b'"' | b'#' | b'<' | b'>' | b'?' | b'`' | b'{' | b'}') {
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
        && b.iter().all(|&c| c.is_ascii_alphanumeric() || c == b'_' || c == b'.')
}

fn borrow_place(expr: &str) -> String {
    if is_field_path(expr) { format!("&({expr})") } else { expr.to_string() }
}

/// `let PAT = EXPR` with EXPR borrowed when it is a field path.
fn if_condition(cond: &str) -> String {
    let Some(rest) = cond.strip_prefix("let").filter(|r| r.starts_with(char::is_whitespace)) else {
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
        Some(i) => format!("let {} = {}", rest[..i].trim(), borrow_place(rest[i + 1..].trim())),
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

    fn template(&mut self, t: &Tpl) {
        self.line(0, &format!("// {}", t.rel));
        self.line(0, "#[doc(hidden)]");
        self.line(0, "#[allow(unused_imports, unused_variables, unused_mut, unused_parens, unused_braces, clippy::all)]");
        self.line(0, &format!("pub mod {} {{", t.module));
        if let Some((m, _)) = &t.user {
            self.line(1, &format!("use super::{m}::*;"));
        }
        if !self.release {
            let chunks: Vec<String> = t.t.chunks.iter().map(|c| lit(c)).collect();
            self.line(1, &format!("static S: [&str; {}] = [{}];", chunks.len(), chunks.join(", ")));
            self.line(1, "#[inline(always)]");
            self.line(1, &format!("fn s(i: usize) -> &'static str {{ ::wisp::rt::chunk({}, i, S[i]) }}", t.id));
        }
        let data = match &t.user {
            Some((m, true)) => format!(", data: &super::{m}::Data"),
            _ => String::new(),
        };
        let sig = match t.kind {
            Kind::Page => format!("pub fn render(__o: &mut ::wisp::Out{data})"),
            Kind::Layout => format!("pub fn render(__o: &mut ::wisp::Out{data}, children: &dyn Fn(&mut ::wisp::Out))"),
            Kind::Error => "pub fn render(__o: &mut ::wisp::Out, status: u16, message: &str)".into(),
        };
        self.line(1, &format!("{sig} {{"));
        let mut cx = Emit { rel: &t.rel, template: &t.t, target: "body", each_depth: 0 };
        self.nodes(&t.t.nodes, 2, &mut cx);
        self.line(1, "}");
        self.line(0, "}");
        self.line(0, "");
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
                    self.line(ind, &format!("{buf}.push_str(s({i}));"));
                }
            }
            Node::Expr { code, quote } => {
                let call = format!("::wisp::rt::text(&mut {buf}, &({}));", code.src);
                if *quote {
                    self.line(ind, &format!("{buf}.push('\"');"));
                    self.code_line(ind, &call, code, cx);
                    self.line(ind, &format!("{buf}.push('\"');"));
                } else {
                    self.code_line(ind, &call, code, cx);
                }
            }
            Node::Html(code) => self.code_line(ind, &format!("::wisp::rt::html(&mut {buf}, &({}));", code.src), code, cx),
            Node::Const(code) => self.code_line(ind, &format!("let {};", code.src), code, cx),
            Node::Render => self.line(ind, "children(__o);"),
            Node::If { branches, otherwise } => {
                for (k, (cond, body)) in branches.iter().enumerate() {
                    let kw = if k == 0 { "if" } else { "} else if" };
                    self.code_line(ind, &format!("{kw} {} {{", if_condition(&cond.src)), cond, cx);
                    self.nodes(body, ind + 1, cx);
                }
                if let Some(o) = otherwise {
                    self.line(ind, "} else {");
                    self.nodes(o, ind + 1, cx);
                }
                self.line(ind, "}");
            }
            Node::Each { iter, pat, index, body, otherwise } => {
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
                self.code_line(ind, &format!("match {} {{", borrow_place(&scrutinee.src)), scrutinee, cx);
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
        }
    }
}

struct Emit<'a> {
    rel: &'a str,
    template: &'a Template,
    target: &'static str,
    each_depth: usize,
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
        assert_eq!(if_condition("let Some(u) = data.user"), "let Some(u) = &(data.user)");
        assert_eq!(if_condition("let Some(u) = find(x)"), "let Some(u) = find(x)");
        assert_eq!(if_condition("let 1..=5 = n"), "let 1..=5 = n");
        assert_eq!(if_condition("a == b"), "a == b");
        assert_eq!(if_condition("letter"), "letter");
    }

    #[test]
    fn path_encoding() {
        assert_eq!(encode_path("über uns"), "%C3%BCber%20uns");
        assert_eq!(encode_path("a-b_c.d~e!$&'()*+,;=:@"), "a-b_c.d~e!$&'()*+,;=:@");
    }
}
