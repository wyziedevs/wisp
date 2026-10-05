//! Discovers routes from the `src/routes` directory tree.

use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Seg {
    Static(String),
    /// `[name]`, or `[name=matcher]`: the name and the matcher.
    Param(String, Option<String>),
    /// `[[name]]`: present or absent.
    Optional(String, Option<String>),
    /// `[...name]`: zero or more segments.
    Rest(String),
}

impl Seg {
    /// The parameter it gives, if it gives one.
    pub fn param(&self) -> Option<&str> {
        match self {
            Seg::Static(_) => None,
            Seg::Param(n, _) | Seg::Optional(n, _) | Seg::Rest(n) => Some(n),
        }
    }
}

/// A `+layout.wisp` (and optional `+layout.rs`).
#[derive(Debug)]
pub struct Layout {
    pub dir: PathBuf,
    pub has_rs: bool,
}

#[derive(Debug)]
pub struct Route {
    pub dir: PathBuf,
    pub segs: Vec<Seg>,
    pub page: bool,
    /// The page file's name: `+page.wisp`, or `+page@.wisp` / `+page@group.wisp`,
    /// which leaves out the layouts above it (all of them, or those above
    /// `group`'s own).
    pub page_file: String,
    /// The page is Markdown: this `+page.md` or `x.md`.
    pub md: Option<PathBuf>,
    pub page_rs: bool,
    /// A `+page.js` (or `+page.ts`) whose `load` runs in the browser: its
    /// name.
    pub page_js: Option<&'static str>,
    pub server: bool,
    /// The `/[id]` its directory's `+server.rs` also serves: handlers that
    /// take an `id` the directory does not give, or a `#[derive(Rest)]`'s.
    pub member: bool,
    /// Indices into `Tree::layouts`, outermost first.
    pub layouts: Vec<usize>,
    /// Index into `Tree::errors` of the nearest `+error.wisp`, if any.
    pub error: Option<usize>,
}

/// A `+error.wisp`, rendered inside the layouts of its own directory.
#[derive(Debug)]
pub struct ErrorPage {
    pub dir: PathBuf,
    pub layouts: Vec<usize>,
}

/// A `@name` folder: a page a layout (`{@render name()}`) draws inside
/// itself, besides its children.
#[derive(Debug)]
pub struct Slot {
    /// The layout beside the folder.
    pub layout: usize,
    pub name: String,
    pub dir: PathBuf,
    /// The route of its `+page.wisp` (`/dashboard/@name`: also served).
    pub route: usize,
}

/// A page under a slot (`@modal/(.)photo/[id]/+page@.wisp`) that a client
/// navigation to another route's URL (`/photo/[id]`) shows in the slot
/// instead, the URL changing and the page staying. A direct load or reload
/// of that URL is the other route's own page.
#[derive(Debug)]
pub struct Intercept {
    /// The route of the intercepting page, at its own URL (`inner`).
    pub route: usize,
    pub slot: String,
    /// The URL pattern intercepted.
    pub target: String,
    /// The intercepting page's own URL pattern, which a navigation fetches.
    pub inner: String,
}

#[derive(Debug, Default)]
pub struct Tree {
    pub intercepts: Vec<Intercept>,
    pub slots: Vec<Slot>,
    /// Sorted by match priority; a route's id is its index.
    pub routes: Vec<Route>,
    pub layouts: Vec<Layout>,
    pub errors: Vec<ErrorPage>,
    /// The param matchers routes use: each name with its `src/params` file,
    /// or `None` for the built-in `int`.
    pub matchers: Vec<(String, Option<PathBuf>)>,
    /// Each `+loading.wisp`: the URL pattern of its folder (empty for the
    /// root) and the file.
    pub loading: Vec<(String, PathBuf)>,
}

/// The segments as a path, none for none: `/blog/[slug]`.
fn prefix(segs: &[Seg]) -> String {
    let mut s = String::new();
    for seg in segs {
        s.push('/');
        match seg {
            Seg::Static(n) => s.push_str(n),
            Seg::Param(n, m) => s.push_str(&format!("[{n}{}]", matcher(m))),
            Seg::Optional(n, m) => s.push_str(&format!("[[{n}{}]]", matcher(m))),
            Seg::Rest(n) => s.push_str(&format!("[...{n}]")),
        }
    }
    s
}

/// `=int` for a matcher, nothing without one.
fn matcher(m: &Option<String>) -> String {
    m.as_ref().map_or(String::new(), |m| format!("={m}"))
}

impl Route {
    /// The route as a URL pattern, for messages: `/blog/[slug]`.
    pub fn pattern(&self) -> String {
        match prefix(&self.segs) {
            p if p.is_empty() => "/".into(),
            p => p,
        }
    }

    /// Whether it has `[[lang=locale]]`, the segment the app's locales go in.
    pub fn has_locale(&self) -> bool {
        let is = |s: &Seg| matches!(s, Seg::Optional(n, Some(m)) if n == "lang" && m == "locale");
        self.segs.iter().any(is)
    }

    pub fn params(&self) -> Vec<&str> {
        self.segs.iter().filter_map(Seg::param).collect()
    }

    /// Every concrete pattern this route matches: optional segments expand
    /// into present/absent variants.
    pub fn expansions(&self) -> Vec<Vec<&Seg>> {
        let mut out: Vec<Vec<&Seg>> = vec![Vec::new()];
        for seg in &self.segs {
            if let Seg::Optional(..) = seg {
                let with: Vec<_> = out
                    .iter()
                    .map(|v| v.iter().copied().chain([seg]).collect())
                    .collect();
                out.extend(with);
            } else {
                out.iter_mut().for_each(|v| v.push(seg));
            }
        }
        out
    }
}

pub const MAX_PARAMS: usize = 8;
/// The deepest path the server routes (`MAX_SEGS` in wisp's http.rs).
const MAX_SEGS: usize = 32;

pub fn scan(routes_dir: &Path) -> Result<Tree, String> {
    let mut tree = Tree::default();
    if routes_dir.is_dir() {
        walk(
            routes_dir,
            routes_dir,
            &mut Vec::new(),
            &mut Vec::new(),
            None,
            &mut tree,
        )?;
    }
    let show = |p: &Path| show(routes_dir, p);

    let mut matchers: Vec<(String, Option<PathBuf>)> = Vec::new();
    for r in &tree.routes {
        let params = r.params();
        if params.len() > MAX_PARAMS {
            return Err(format!(
                "{}: more than {MAX_PARAMS} parameters",
                r.pattern()
            ));
        }
        if r.segs.len() > MAX_SEGS {
            return Err(format!(
                "{}: deeper than {MAX_SEGS} segments, which no request reaches",
                show(&r.dir)
            ));
        }
        if r.segs.iter().filter(|s| matches!(s, Seg::Rest(_))).count() > 1 {
            return Err(format!(
                "{}: at most one [...rest] segment per route",
                r.pattern()
            ));
        }
        for (i, p) in params.iter().enumerate() {
            if params[..i].contains(p) {
                return Err(format!("{}: parameter `{p}` appears twice", r.pattern()));
            }
        }
        for file in ["+page.rs", "+server.rs"] {
            let path = r.dir.join(file);
            let Ok(src) = fs::read_to_string(&path) else {
                continue;
            };
            if let Some(name) = unknown_param(&src, &params) {
                return Err(format!(
                    "{}: `cx.param(\"{name}\")`, but the route's parameters are {params:?}",
                    show(&path)
                ));
            }
        }
        for seg in &r.segs {
            let (Seg::Param(n, Some(m)) | Seg::Optional(n, Some(m))) = seg else {
                continue;
            };
            if m == "locale" && n != "lang" {
                return Err(format!(
                    "{}: the locale's parameter is `lang`: [[lang=locale]]",
                    show(&r.dir)
                ));
            }
            if matchers.iter().any(|(n, _)| n == m) {
                continue;
            }
            let file = routes_dir.with_file_name("params").join(format!("{m}.rs"));
            let file = file.is_file().then_some(file);
            if file.is_none() && m != "int" && m != "locale" {
                return Err(format!(
                    "{}: no param matcher `{m}`: add src/params/{m}.rs with `fn matches(s: &str) -> bool` (`int` and `locale` are built in)",
                    show(&r.dir)
                ));
            }
            matchers.push((m.clone(), file));
        }
    }
    matchers.sort();
    tree.matchers = matchers;

    // Route order only makes ids and generated code deterministic; matching
    // order comes from `arms`.
    tree.routes.sort_by(|a, b| {
        priority(
            &a.segs.iter().collect::<Vec<_>>(),
            &b.segs.iter().collect::<Vec<_>>(),
        )
    });

    // Each slot's page, by the route's place after sorting.
    let slots = std::mem::take(&mut tree.slots);
    for mut s in slots {
        s.route = tree
            .routes
            .iter()
            .position(|r| r.dir == s.dir && r.page && r.md.is_none() && !r.member)
            .ok_or_else(|| format!("{}: a slot is a +page.wisp", show(&s.dir)))?;
        tree.slots.push(s);
    }
    // A `(.)name` segment under a `@slot`: what it intercepts.
    for (id, r) in tree.routes.iter().enumerate() {
        let Some(j) = r
            .segs
            .iter()
            .position(|s| matches!(s, Seg::Static(n) if n.starts_with("(.")))
        else {
            continue;
        };
        let at = |why: &str| format!("{}: {why}", show(&r.dir));
        let Some(k) = r.segs[..j]
            .iter()
            .rposition(|s| matches!(s, Seg::Static(n) if n.starts_with('@')))
        else {
            return Err(at(
                "an intercepting page (`(.)name`) goes inside a `@slot` folder",
            ));
        };
        let Seg::Static(mark) = &r.segs[j] else {
            continue;
        };
        let Seg::Static(slot) = &r.segs[k] else {
            continue;
        };
        if r.page_file != "+page@.wisp" || !r.page {
            return Err(at(
                "an intercepting page is `+page@.wisp`: it is drawn inside the page that is there, without the layouts above it",
            ));
        }
        // `(.)name` is 0 levels up, `(..)name` 1, `(...)name` the root.
        let (dots, name) = mark[1..].split_once(')').unwrap_or_default();
        if !(1..=3).contains(&dots.len()) || dots.contains(|c| c != '.') || name.is_empty() {
            return Err(at(
                "name the page it intercepts: `(.)name`, `(..)name` or `(...)name`",
            ));
        }
        let up = dots.len() - 1;
        let mut target: Vec<Seg> = (r.segs[..k].iter())
            .filter(|s| !matches!(s, Seg::Static(n) if n.starts_with('@')))
            .cloned()
            .collect();
        match up {
            2 => target.clear(),
            n => target.truncate(target.len().saturating_sub(n)),
        }
        target.push(Seg::Static(name.to_string()));
        target.extend(r.segs[j + 1..].iter().cloned());
        let (target, inner) = (prefix(&target), prefix(&r.segs));
        tree.intercepts.push(Intercept {
            route: id,
            slot: slot[1..].to_string(),
            target,
            inner,
        });
    }
    // What a page intercepts is a page of the app: else it would never be drawn.
    let shape = |p: &str| {
        let seg = |s: &str| {
            if s.starts_with("[...") {
                "[...]"
            } else if s.starts_with('[') {
                "[]"
            } else {
                s
            }
            .to_string()
        };
        p.split('/').map(seg).collect::<Vec<_>>()
    };
    for i in &tree.intercepts {
        let want = shape(&i.target);
        if !tree
            .routes
            .iter()
            .any(|r| r.page && shape(&r.pattern()) == want)
        {
            return Err(format!(
                "{}: it intercepts {}, which is not a page of the app",
                show(&tree.routes[i.route].dir),
                i.target
            ));
        }
    }
    // Two routes that can match the exact same URLs are ambiguous.
    let mut seen: Vec<(Vec<String>, usize)> = Vec::new();
    for (id, r) in tree.routes.iter().enumerate() {
        for exp in r.expansions() {
            let key: Vec<String> = exp
                .iter()
                .map(|s| match s {
                    Seg::Static(n) => format!("s:{n}"),
                    Seg::Param(_, m) | Seg::Optional(_, m) => format!("p{}", matcher(m)),
                    Seg::Rest(_) => "r".into(),
                })
                .collect();
            match seen.iter().find(|(k, _)| *k == key) {
                Some(&(_, other)) if other != id => {
                    return Err(format!(
                        "routes {} ({}) and {} ({}) match the same URLs",
                        tree.routes[other].pattern(),
                        show(&tree.routes[other].dir),
                        r.pattern(),
                        show(&r.dir)
                    ));
                }
                Some(_) => {}
                None => seen.push((key, id)),
            }
        }
    }
    Ok(tree)
}

impl Tree {
    /// Every concrete pattern with its route id, most specific first. This is
    /// the arm order of the generated router `match`.
    pub fn arms(&self) -> Vec<(Vec<&Seg>, usize)> {
        let mut arms: Vec<_> = self
            .routes
            .iter()
            .enumerate()
            .flat_map(|(id, r)| r.expansions().into_iter().map(move |e| (e, id)))
            .collect();
        arms.sort_by(|a, b| priority(&a.0, &b.0)); // stable: ties keep route order
        arms
    }
}

/// The first `cx.param("name")` in `src` (comment lines skipped) whose name
/// the route lacks: `Cx::param` panics on one, so the build stops instead.
fn unknown_param<'s>(src: &'s str, params: &[&str]) -> Option<&'s str> {
    const CALL: &str = "cx.param(\"";
    src.lines()
        .filter(|l| !l.trim_start().starts_with("//"))
        .flat_map(|l| {
            l.match_indices(CALL)
                .map(move |(i, _)| &l[i + CALL.len()..])
        })
        .filter_map(|rest| rest.split_once('"').map(|(name, _)| name))
        .find(|name| !params.contains(name))
}

/// `src/routes/...`, for messages.
fn show(routes_dir: &Path, p: &Path) -> String {
    let rel = p
        .strip_prefix(routes_dir)
        .unwrap_or(p)
        .to_string_lossy()
        .replace('\\', "/");
    if rel.is_empty() {
        "src/routes".into()
    } else {
        format!("src/routes/{rel}")
    }
}

/// A file an editor keeps beside the one being edited: a swap file, a
/// backup, a lock. Never part of the app.
pub fn editor_temp(name: &str) -> bool {
    name.starts_with(['.', '#'])
        || name.ends_with('~')
        || [".swp", ".swx", ".tmp", ".bak", ".orig"]
            .iter()
            .any(|e| name.ends_with(e))
        || name.contains("___jb_")
        || name == "4913"
}

fn walk(
    root: &Path,
    dir: &Path,
    segs: &mut Vec<Seg>,
    layouts: &mut Vec<usize>,
    error: Option<usize>,
    tree: &mut Tree,
) -> Result<(), String> {
    let show = |p: &Path| show(root, p);
    let mut files = Vec::new();
    let mut dirs = Vec::new();
    // `x.md`: a Markdown page at `x`.
    let mut mds = Vec::new();
    for e in fs::read_dir(dir).map_err(|e| format!("{}: {e}", show(dir)))? {
        let e = e.map_err(|e| format!("{}: {e}", show(dir)))?;
        // A name lands in the generated code's `//` comments and paths, so
        // it must be UTF-8 with no control character (a newline would end
        // the comment and turn the rest of the name into code).
        let Some(name) = e.file_name().to_str().map(str::to_string) else {
            return Err(format!(
                "{}: a file or folder name here is not UTF-8; rename it",
                show(dir)
            ));
        };
        if name.chars().any(char::is_control) {
            return Err(format!(
                "{}: {name:?} has a control character (a newline or tab); rename it",
                show(dir)
            ));
        }
        let is_dir = e.file_type().map_err(|e| e.to_string())?.is_dir();
        // `/.well-known/...` is a URL apps serve; no editor leaves that folder.
        if editor_temp(&name) && !(is_dir && name == ".well-known") {
            continue;
        }
        if is_dir {
            dirs.push(name);
        } else if name.starts_with('+') {
            files.push(name);
        } else if let Some(stem) = name.strip_suffix(".md") {
            match parse_segment(stem) {
                Ok(Some(Seg::Static(_))) => mds.push(name),
                _ => {
                    return Err(format!(
                        "{}: a Markdown page's name is its URL segment: letters, digits, `-`, `_`, `.`",
                        show(&dir.join(&name))
                    ));
                }
            }
        } else if name.ends_with(".wisp")
            || matches!(name.as_str(), "page.rs" | "layout.rs" | "server.rs")
        {
            // Only `+` files are routes; this one would be ignored.
            return Err(format!(
                "{}: route files start with `+`; did you mean `+{name}`?",
                show(&dir.join(&name))
            ));
        }
    }
    files.sort();
    dirs.sort();
    mds.sort();

    let reset = files
        .iter()
        .find_map(|f| reset_of(f).map(|g| (f.clone(), g.to_string())));
    let has = |f: &str| files.iter().any(|x| x == f) || (f == "+page.wisp" && reset.is_some());
    for f in &files {
        const KNOWN: [&str; 10] = [
            "+page.wisp",
            "+page.rs",
            "+page.js",
            "+page.ts",
            "+page.md",
            "+layout.wisp",
            "+layout.rs",
            "+error.wisp",
            "+loading.wisp",
            "+server.rs",
        ];
        if !KNOWN.contains(&f.as_str()) && reset_of(f).is_none() {
            return Err(format!(
                "{}: unknown route file (expected one of {})",
                show(&dir.join(f)),
                KNOWN.join(", ")
            ));
        }
    }
    if has("+page.md") && has("+page.wisp") {
        return Err(format!(
            "{}: +page.md and +page.wisp are one page; keep one",
            show(dir)
        ));
    }
    if has("+page.rs") && !has("+page.wisp") {
        return Err(format!(
            "{}: +page.rs needs a +page.wisp next to it",
            show(dir)
        ));
    }
    let page_js = ["+page.js", "+page.ts"].into_iter().find(|f| has(f));
    if let Some(f) = page_js
        && !has("+page.wisp")
    {
        return Err(format!("{}: {f} needs a +page.wisp next to it", show(dir)));
    }
    if has("+page.js") && has("+page.ts") {
        return Err(format!(
            "{}: +page.js and +page.ts are one file; keep one",
            show(dir)
        ));
    }
    if has("+layout.rs") && !has("+layout.wisp") {
        return Err(format!(
            "{}: +layout.rs needs a +layout.wisp next to it",
            show(dir)
        ));
    }

    let depth = layouts.len();
    if has("+layout.wisp") {
        tree.layouts.push(Layout {
            dir: dir.to_path_buf(),
            has_rs: has("+layout.rs"),
        });
        layouts.push(tree.layouts.len() - 1);
    }
    if has("+loading.wisp") {
        tree.loading.push((prefix(segs), dir.join("+loading.wisp")));
    }
    let mut error = error;
    if has("+error.wisp") {
        tree.errors.push(ErrorPage {
            dir: dir.to_path_buf(),
            layouts: layouts.clone(),
        });
        error = Some(tree.errors.len() - 1);
    }
    let page_md = has("+page.md").then(|| dir.join("+page.md"));
    // `+page@group.wisp`: the layouts above `group`'s directory (it too) stay.
    let (page_file, page_layouts) = match &reset {
        None => ("+page.wisp".to_string(), layouts.clone()),
        Some((f, group)) => {
            let keep = |l: &usize| {
                tree.layouts[*l]
                    .dir
                    .file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| {
                        n == group
                            || n.strip_prefix('(').and_then(|n| n.strip_suffix(')')) == Some(group)
                    })
            };
            let at = match group.is_empty() {
                true => Some(0),
                false => layouts.iter().rposition(keep).map(|i| i + 1),
            };
            let Some(at) = at else {
                return Err(format!(
                    "{}: `{f}` resets to the layout of `{group}`, and no directory above it has a +layout.wisp of that name",
                    show(&dir.join(f))
                ));
            };
            (f.clone(), layouts[..at].to_vec())
        }
    };
    // Its endpoints: a `+server.rs`, or the `mod server` of the page's block.
    let server = match has("+server.rs") {
        true => Some(crate::read_source(&dir.join("+server.rs")).unwrap_or_default()),
        false if has("+page.wisp") => crate::block_server(&dir.join(&page_file)).map(|s| s.0),
        false => None,
    };
    let (collection, member) = match server {
        Some(src) => server_shape(&src, segs),
        None => (false, None),
    };
    if has("+page.wisp") || page_md.is_some() || collection {
        tree.routes.push(Route {
            dir: dir.to_path_buf(),
            segs: segs.clone(),
            page: has("+page.wisp") || page_md.is_some(),
            page_file: page_file.clone(),
            md: page_md,
            page_rs: has("+page.rs"),
            page_js,
            server: collection,
            member: false,
            layouts: page_layouts,
            error,
        });
    }
    if let Some(matcher) = member {
        let mut segs = segs.clone();
        segs.push(Seg::Param("id".into(), matcher));
        tree.routes.push(Route {
            dir: dir.to_path_buf(),
            segs,
            page: false,
            // Not served, but its `mod server` may hold this route's handlers.
            page_file: match has("+page.wisp") {
                true => page_file.clone(),
                false => String::new(),
            },
            md: None,
            page_rs: false,
            page_js: None,
            server: true,
            member: true,
            layouts: layouts.clone(),
            error,
        });
    }
    for name in mds {
        let stem = &name[..name.len() - 3];
        if segs.is_empty() && (stem == "_app" || stem == "_wisp") {
            return Err(format!(
                "{}: /{stem} is reserved for Wisp's own files; choose another name",
                show(&dir.join(&name))
            ));
        }
        let mut segs = segs.clone();
        segs.push(Seg::Static(stem.to_string()));
        tree.routes.push(Route {
            dir: dir.to_path_buf(),
            segs,
            page: true,
            page_file: String::new(),
            md: Some(dir.join(&name)),
            page_rs: false,
            page_js: None,
            server: false,
            member: false,
            layouts: layouts.clone(),
            error,
        });
    }

    for d in dirs {
        if let Some(name) = d.strip_prefix('@') {
            let at = show(&dir.join(&d));
            if !crate::ty::is_ident(name) || matches!(name, "children" | "data" | "cx") {
                return Err(format!(
                    "{at}: `{name}` is not a slot name: a Rust name that is not `children`, `data` or `cx`"
                ));
            }
            if layouts.len() == depth {
                return Err(format!(
                    "{at}: a slot needs a +layout.wisp in the folder above it, to draw it with {{@render {name}()}}"
                ));
            }
            tree.slots.push(Slot {
                layout: layouts[depth],
                name: name.to_string(),
                dir: dir.join(&d),
                route: usize::MAX,
            });
        }
        let seg = parse_segment(&d).map_err(|e| format!("{}: {e}", show(&dir.join(&d))))?;
        if segs.is_empty() && matches!(&seg, Some(Seg::Static(s)) if s == "_app" || s == "_wisp") {
            return Err(format!(
                "{}: /{d} is reserved for Wisp's own files; choose another name",
                show(&dir.join(&d))
            ));
        }
        let pushed = seg.is_some();
        if let Some(s) = seg {
            segs.push(s);
        }
        walk(root, &dir.join(&d), segs, layouts, error, tree)?;
        if pushed {
            segs.pop();
        }
    }
    layouts.truncate(depth);
    Ok(())
}

/// The handler names of a `+server.rs`: HTTP methods, and `list` for a
/// GET of the route when `get` is its `/[id]`'s.
pub const HANDLERS: [&str; 6] = ["get", "post", "put", "patch", "delete", "list"];

/// A file's `const RUNTIME: wisp::Runtime = wisp::Runtime::Edge;`: whether it
/// is Edge, none without one. The error is a message for the file's line.
pub fn edge(items: &crate::rust_scan::Items) -> Result<Option<bool>, (usize, String)> {
    let Some(c) = items.constant("RUNTIME") else {
        return Ok(None);
    };
    let value = c.value.replace(' ', "");
    let name = value.rsplit("::").next().unwrap_or("");
    let path = value.trim_end_matches(name).trim_end_matches("::");
    let ok = !c.is_static
        && c.ty.replace(' ', "").rsplit("::").next() == Some("Runtime")
        && matches!(path, "wisp::Runtime" | "::wisp::Runtime" | "Runtime");
    match (ok, name) {
        (true, "Edge") => Ok(Some(true)),
        (true, "Node") => Ok(Some(false)),
        _ => Err((
            c.line,
            "the build reads `RUNTIME`: write `const RUNTIME: wisp::Runtime = wisp::Runtime::Edge;` (or `Node`), as a literal".into(),
        )),
    }
}

/// Whether handler `f` serves its route's `/[id]`: it takes an `id` that
/// the route (`segs`) does not have.
pub fn is_member(f: &crate::rust_scan::FnItem, segs: &[Seg]) -> bool {
    let routed = segs
        .iter()
        .any(|s| matches!(s, Seg::Param(n, _) | Seg::Optional(n, _) | Seg::Rest(n) if n == "id"));
    !routed
        && f.inputs()
            .is_ok_and(|ins| ins.iter().any(|(n, _)| *n == "id"))
}

/// The file's `#[derive(Rest)]` type, which it serves.
pub fn rest_type(items: &crate::rust_scan::Items) -> Option<&str> {
    let t = items
        .types
        .iter()
        .find(|t| t.derives.iter().any(|d| d == "Rest"));
    t.map(|t| t.name.as_str())
}

/// What the endpoints `src` (a `+server.rs`, or a page's `mod server`)
/// serve: its route (true unless every handler is its `/[id]`'s), and that
/// `/[id]`, if any, with the matcher `int` when every `id` it takes is an
/// integer. Code that does not scan is left to `codegen`, which says what
/// is wrong.
fn server_shape(src: &str, segs: &[Seg]) -> (bool, Option<Option<String>>) {
    let Ok(items) = crate::rust_scan::scan(src) else {
        return (true, None);
    };
    let handlers: Vec<_> = items
        .fns
        .iter()
        .filter(|f| HANDLERS.contains(&f.name.as_str()))
        .collect();
    let routed = segs
        .iter()
        .any(|s| matches!(s, Seg::Param(n, _) | Seg::Optional(n, _) | Seg::Rest(n) if n == "id"));
    let rest = rest_type(&items).is_some() && !routed;
    let members: Vec<_> = handlers.iter().filter(|f| is_member(f, segs)).collect();
    let collection = rest || handlers.len() > members.len() || handlers.is_empty();
    if !rest && members.is_empty() {
        return (collection, None);
    }
    let int = members.iter().all(|f| {
        f.inputs().is_ok_and(|ins| {
            ins.iter()
                .any(|(n, t)| *n == "id" && crate::ty::is_integer(t))
        })
    });
    (collection, Some(int.then(|| "int".to_string())))
}

/// `None` for `(group)` directories, which do not appear in the URL.
/// `group` of `+page@group.wisp` (empty for `+page@.wisp`).
fn reset_of(file: &str) -> Option<&str> {
    file.strip_prefix("+page@")?.strip_suffix(".wisp")
}

pub fn parse_segment(name: &str) -> Result<Option<Seg>, String> {
    if name.chars().any(char::is_control) {
        return Err(format!("{name:?} has a control character; rename it"));
    }
    let ident = |s: &str| -> Result<String, String> {
        if !crate::ty::is_ident(s) {
            return Err(format!("`{s}` is not a valid parameter name"));
        }
        if crate::ty::is_unrawable(s) {
            return Err(format!(
                "`{s}` is a Rust keyword, so it cannot name a parameter"
            ));
        }
        Ok(s.to_string())
    };
    // The generated code has locals of its own (`cx`, `__o`): a parameter
    // of that name would be one of them.
    let pname = |s: &str| -> Result<String, String> {
        let n = ident(s)?;
        if n == "cx" || n.starts_with("__") {
            return Err(format!(
                "`{n}` is reserved for the generated code; rename the parameter"
            ));
        }
        Ok(n)
    };
    // `name=matcher`: the matcher is a file in src/params, or `int`.
    let param = |s: &str| -> Result<(String, Option<String>), String> {
        match s.split_once('=') {
            Some((n, m)) => {
                let m = ident(m).map_err(|_| format!("`{m}` is not a valid matcher name"))?;
                Ok((pname(n)?, Some(m)))
            }
            None => Ok((pname(s)?, None)),
        }
    };
    if let Some(inner) = name.strip_prefix('(').and_then(|s| s.strip_suffix(')')) {
        if inner.is_empty() || inner.contains(['(', ')']) {
            return Err(format!(
                "`{name}` is not a group: a group is one name in parentheses, like `(marketing)`"
            ));
        }
        return Ok(None);
    }
    if let Some(inner) = name.strip_prefix("[[").and_then(|s| s.strip_suffix("]]")) {
        let (n, m) = param(inner)?;
        return Ok(Some(Seg::Optional(n, m)));
    }
    if let Some(inner) = name.strip_prefix("[...").and_then(|s| s.strip_suffix(']')) {
        if inner.contains('=') {
            return Err(
                "matchers go on [name=matcher] and [[name=matcher]], not on [...rest]".into(),
            );
        }
        return Ok(Some(Seg::Rest(pname(inner)?)));
    }
    if let Some(inner) = name.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
        let (n, m) = param(inner)?;
        return Ok(Some(Seg::Param(n, m)));
    }
    // `(.)photo`, `(..)photo`, `(...)photo`: a page that intercepts that
    // route (see `Tree::intercepts`); its URL, inside a slot, is its own.
    if let Some(rest) = ["(...)", "(..)", "(.)"]
        .iter()
        .find_map(|p| name.strip_prefix(p))
        && !rest.is_empty()
        && !rest.contains(['[', ']', '(', ')'])
    {
        return Ok(Some(Seg::Static(name.to_string())));
    }
    if name.contains(['[', ']', '(', ')']) {
        return Err("mixed static and dynamic text in one segment is not supported".into());
    }
    // The path is matched as the browser sends it, so a name it would
    // percent-encode (a space, `%`, `#`, a non-ASCII letter) never matches.
    if let Some(c) = name
        .chars()
        .find(|&c| !(c.is_ascii_alphanumeric() || "-._~!$&'*+,;=:@".contains(c)))
    {
        return Err(format!(
            "`{c}` in the folder name `{name}` would be percent-encoded in a URL, so the route could never match; use letters, digits and `-._~`"
        ));
    }
    Ok(Some(Seg::Static(name.to_string())))
}

/// Static beats param beats optional beats rest, left to right, and one
/// with a matcher beats one without. When one pattern is a prefix of the
/// other, the shorter (more exact) one goes first.
fn priority(a: &[&Seg], b: &[&Seg]) -> std::cmp::Ordering {
    let rank = |s: &Seg| match s {
        Seg::Static(_) => 0,
        Seg::Param(_, Some(_)) => 1,
        Seg::Param(_, None) => 2,
        Seg::Optional(_, Some(_)) => 3,
        Seg::Optional(_, None) => 4,
        Seg::Rest(_) => 5,
    };
    for (&x, &y) in a.iter().zip(b) {
        let ord = rank(x).cmp(&rank(y)).then_with(|| match (x, y) {
            (Seg::Static(p), Seg::Static(q)) => p.cmp(q),
            (Seg::Param(_, p) | Seg::Optional(_, p), Seg::Param(_, q) | Seg::Optional(_, q)) => {
                p.cmp(q)
            }
            _ => std::cmp::Ordering::Equal,
        });
        if ord.is_ne() {
            return ord;
        }
    }
    a.len().cmp(&b.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("wisp-routes-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        d
    }

    fn touch(root: &Path, rel: &str) {
        let p = root.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, "").unwrap();
    }

    #[test]
    fn a_parameter_is_not_a_keyword() {
        for bad in ["[self]", "[_]", "[...crate]", "[[super]]"] {
            let e = parse_segment(bad).unwrap_err();
            assert!(e.contains("Rust keyword"), "{bad}: {e}");
        }
        assert!(parse_segment("[kind]").is_ok());
        assert!(parse_segment("[type]").is_ok());
        for bad in ["a b", "a%b", "日本", "a#b", "a\\b"] {
            assert!(parse_segment(bad).is_err(), "{bad}");
        }
        for ok in ["a-b_c.d", "@modal", ".well-known", "a+b"] {
            assert!(parse_segment(ok).is_ok(), "{ok}");
        }
    }

    /// `/.well-known/...` is a URL apps serve (webfinger, OIDC, passkeys):
    /// its folder is a route, not an editor's dropping.
    #[test]
    fn a_well_known_folder_is_a_route() {
        let root = tmp("wellknown");
        touch(&root, ".well-known/webfinger/+server.rs");
        touch(&root, ".git/x/+page.wisp");
        touch(&root, "a/.swp/+page.wisp");
        let pats: Vec<_> = scan(&root)
            .unwrap()
            .routes
            .iter()
            .map(Route::pattern)
            .collect();
        assert_eq!(pats, ["/.well-known/webfinger"]);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn a_control_character_in_a_name_is_an_error() {
        for bad in ["(a\nb)", "(.)a\nb", "(x\r)", "a\tb", "[[a\n]]"] {
            assert!(parse_segment(bad).is_err(), "{bad:?}");
        }
        // On disk too (where the OS allows it): the name reaches `//`
        // comments in wisp.rs, and a newline would end one.
        let root = tmp("ctrl");
        let _ = fs::create_dir_all(&root);
        if fs::create_dir_all(root.join("(a\nfn x() {})")).is_ok() {
            touch(&root, "(a\nfn x() {})/+page.wisp");
            assert!(scan(&root).unwrap_err().contains("control character"));
        }
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn fuzzed_segments_never_panic() {
        let mut x = 0x9E37_79B9_7F4A_7C15u64;
        let pool: Vec<char> = "[]().=@a_1 \"\\*/\n\r\u{202e}日-".chars().collect();
        for _ in 0..20_000 {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            let len = (x % 9) as usize;
            let name: String = (0..len)
                .map(|i| pool[((x >> (i * 5)) % pool.len() as u64) as usize])
                .collect();
            if let Ok(Some(
                Seg::Static(n) | Seg::Param(n, _) | Seg::Optional(n, _) | Seg::Rest(n),
            )) = parse_segment(&name)
            {
                assert!(!n.contains(['\n', '\r']), "{name:?}");
            }
        }
    }

    #[test]
    fn handlers_with_an_id_make_a_member_route() {
        let root = tmp("member");
        let write = |rel: &str, src: &str| {
            let p = root.join(rel);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(p, src).unwrap();
        };
        write(
            "a/+server.rs",
            "fn list() {}
fn get(id: u64) {}",
        );
        write("b/+server.rs", "fn delete(id: String) {}");
        write("c/+server.rs", "#[derive(wisp::Rest)]\nstruct R { a: u8 }");
        write("d/[id]/+server.rs", "fn get(id: u64) {}");
        let t = scan(&root).unwrap();
        let _ = fs::remove_dir_all(&root);
        let shape: Vec<(String, bool)> = t.routes.iter().map(|r| (r.pattern(), r.member)).collect();
        for want in [
            ("/a", false),
            ("/a/[id=int]", true),
            ("/b/[id]", true),
            ("/c", false),
            ("/c/[id=int]", true),
            ("/d/[id]", false),
        ] {
            assert!(
                shape.contains(&(want.0.to_string(), want.1)),
                "{want:?}: {shape:?}"
            );
        }
        assert!(
            !shape.iter().any(|(p, _)| p == "/b"),
            "no handler for /b: {shape:?}"
        );
    }

    #[test]
    fn scans_and_sorts() {
        let root = tmp("sort");
        for f in [
            "+layout.wisp",
            "+page.wisp",
            "+error.wisp",
            "about/+page.wisp",
            "blog/+layout.wisp",
            "blog/+layout.rs",
            "blog/+page.wisp",
            "blog/[slug]/+page.wisp",
            "blog/[slug]/+page.rs",
            "blog/new/+page.wisp",
            "docs/[...path]/+page.wisp",
            "docs/intro/+page.wisp",
            "(marketing)/pricing/+page.wisp",
            "api/items/+server.rs",
            "[[lang]]/hello/+page.wisp",
            "[slug]/+page.wisp",
        ] {
            touch(&root, f);
        }
        let t = scan(&root).unwrap();
        let pats: Vec<_> = t.routes.iter().map(|r| r.pattern()).collect();
        assert_eq!(
            pats,
            [
                "/",
                "/about",
                "/api/items",
                "/blog",
                "/blog/new",
                "/blog/[slug]",
                "/docs/intro",
                "/docs/[...path]",
                "/pricing",
                "/[slug]",
                "/[[lang]]/hello",
            ]
        );
        let slug = &t.routes[5];
        assert_eq!(slug.layouts.len(), 2);
        assert!(slug.page_rs && slug.error == Some(0));
        assert!(t.layouts[1].has_rs);
        assert!(t.routes[2].server && !t.routes[2].page);

        // `/hello` must reach the optional-lang route before `/[slug]` grabs it.
        let arms: Vec<_> = t
            .arms()
            .iter()
            .map(|(_, id)| t.routes[*id].pattern())
            .collect();
        let hello = arms.iter().position(|p| p == "/[[lang]]/hello").unwrap();
        let slug = arms.iter().position(|p| p == "/[slug]").unwrap();
        assert!(hello < slug, "{arms:?}");
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn misspelled_param_is_a_build_error() {
        let p = ["slug", "id"];
        assert_eq!(unknown_param("let s = cx.param(\"slug\");", &p), None);
        assert_eq!(
            unknown_param("// cx.param(\"x\")\nlet a = cx.param(\"id\");", &p),
            None
        );
        assert_eq!(
            unknown_param("let a = (cx.param(\"id\"), cx.param(\"slgu\"));", &p),
            Some("slgu")
        );
        let root = tmp("misspelled");
        let routes = root.join("routes");
        touch(&routes, "[slug]/+page.wisp");
        let dir = routes.join("[slug]");
        fs::write(
            dir.join("+page.rs"),
            "fn load(cx: &Cx) { cx.param(\"slgu\"); }",
        )
        .unwrap();
        let err = scan(&routes).unwrap_err();
        assert!(
            err.contains("cx.param(\"slgu\")") && err.contains("+page.rs"),
            "{err}"
        );
    }

    #[test]
    fn matchers() {
        let root = tmp("matchers");
        let routes = root.join("routes");
        for f in [
            "[slug]/+page.wisp",
            "[id=int]/+page.wisp",
            "[w=word]/+page.wisp",
            "a/[[n=int]]/+page.wisp",
        ] {
            touch(&routes, f);
        }
        touch(&root, "params/word.rs");
        let t = scan(&routes).unwrap();
        assert_eq!(
            t.matchers,
            [
                ("int".to_string(), None),
                (
                    "word".to_string(),
                    Some(root.join("params").join("word.rs"))
                )
            ]
        );
        // Matched params go before unmatched ones.
        let arms: Vec<_> = t
            .arms()
            .iter()
            .map(|(_, id)| t.routes[*id].pattern())
            .collect();
        assert_eq!(
            arms,
            [
                "/a/[[n=int]]",
                "/a/[[n=int]]",
                "/[id=int]",
                "/[w=word]",
                "/[slug]"
            ]
        );

        touch(&routes, "b/[x=nope]/+page.wisp");
        let err = scan(&routes).unwrap_err();
        assert!(
            err.contains("no param matcher `nope`") && err.contains("src/params/nope.rs"),
            "{err}"
        );
        fs::remove_dir_all(&root).unwrap();

        for (dir, want) in [
            ("[...p=int]", "not on [...rest]"),
            ("[a=]", "not a valid matcher"),
        ] {
            let root = tmp("matcher-bad");
            touch(&root, &format!("{dir}/+page.wisp"));
            let err = scan(&root).unwrap_err();
            assert!(err.contains(want), "{dir}: {err}");
            fs::remove_dir_all(&root).unwrap();
        }
    }

    #[test]
    fn slots_and_the_pages_that_intercept() {
        let root = tmp("slots");
        for f in [
            "+layout.wisp",
            "+page.wisp",
            "gal/+layout.wisp",
            "gal/+page.wisp",
            "gal/@modal/+page.wisp",
            "gal/@modal/(.)photo/[id]/+page@.wisp",
            "gal/@modal/(..)about/+page@.wisp",
            "gal/@modal/(...)top/x/+page@.wisp",
            "gal/photo/[id]/+page.wisp",
            "about/+page.wisp",
            "top/x/+page.wisp",
        ] {
            touch(&root, f);
        }
        let t = scan(&root).unwrap();
        assert_eq!(t.slots.len(), 1);
        let s = &t.slots[0];
        assert_eq!(
            (s.name.as_str(), t.routes[s.route].pattern().as_str()),
            ("modal", "/gal/@modal")
        );
        let mut cuts: Vec<(&str, &str)> = t
            .intercepts
            .iter()
            .map(|i| (i.target.as_str(), i.inner.as_str()))
            .collect();
        cuts.sort();
        assert_eq!(
            cuts,
            [
                ("/about", "/gal/@modal/(..)about"),
                ("/gal/photo/[id]", "/gal/@modal/(.)photo/[id]"),
                ("/top/x", "/gal/@modal/(...)top/x"),
            ]
        );
        fs::remove_dir_all(&root).unwrap();
        // A dot in the name is the name's, not another level up.
        let root = tmp("slot-dots");
        for f in [
            "+layout.wisp",
            "@m/+page.wisp",
            "v1.0/+page.wisp",
            "@m/(.)v1.0/+page@.wisp",
        ] {
            touch(&root, f);
        }
        assert_eq!(scan(&root).unwrap().intercepts[0].target, "/v1.0");
        fs::remove_dir_all(&root).unwrap();
        for (files, want) in [
            (vec!["@m/+page.wisp"], "a slot needs a +layout.wisp"),
            (
                vec!["+layout.wisp", "@children/+page.wisp"],
                "is not a slot name",
            ),
            (
                vec!["+layout.wisp", "@m/x/+page.wisp"],
                "a slot is a +page.wisp",
            ),
            (
                vec!["+layout.wisp", "x/(.)p/+page@.wisp"],
                "inside a `@slot` folder",
            ),
            (
                vec!["+layout.wisp", "@m/(.)p/+page@.wisp"],
                "a slot is a +page.wisp",
            ),
            (
                vec!["+layout.wisp", "@m/+page.wisp", "@m/(.)p/+page.wisp"],
                "is `+page@.wisp`",
            ),
        ] {
            let root = tmp("slot-bad");
            for f in &files {
                touch(&root, f);
            }
            let err = scan(&root).unwrap_err();
            assert!(err.contains(want), "{files:?}: {err}");
            fs::remove_dir_all(&root).unwrap();
        }
    }

    #[test]
    fn rejects_conflicts_and_typos() {
        let root = tmp("conflict");
        touch(&root, "[a]/+page.wisp");
        touch(&root, "[b]/+page.wisp");
        assert!(scan(&root).unwrap_err().contains("match the same URLs"));
        fs::remove_dir_all(&root).unwrap();

        let root = tmp("typo");
        touch(&root, "+Page.wisp");
        assert!(scan(&root).unwrap_err().contains("unknown route file"));
        fs::remove_dir_all(&root).unwrap();

        let root = tmp("optional");
        touch(&root, "+page.wisp");
        touch(&root, "[[lang]]/+page.wisp");
        let err = scan(&root).unwrap_err();
        assert!(
            err.contains("match the same URLs") && err.contains("(src/routes)"),
            "{err}"
        );
        fs::remove_dir_all(&root).unwrap();

        for (file, want) in [
            ("blog/page.wisp", "did you mean `+page.wisp`"),
            ("blog/server.rs", "did you mean `+server.rs`"),
            ("_app/+page.wisp", "reserved"),
            ("(g)/_wisp/+page.wisp", "reserved"),
            ("(a)b(c)/+page.wisp", "not a group"),
        ] {
            let root = tmp("mistake");
            touch(&root, file);
            let err = scan(&root).unwrap_err();
            assert!(err.contains(want), "{file}: {err}");
            fs::remove_dir_all(&root).unwrap();
        }

        // Markdown: `+page.md`, and a page per `x.md`.
        let root = tmp("md");
        touch(&root, "blog/+page.md");
        touch(&root, "blog/hello.md");
        touch(&root, "(private)/notes.md");
        let t = scan(&root).unwrap();
        let pats: Vec<_> = t
            .routes
            .iter()
            .map(|r| (r.pattern(), r.md.is_some()))
            .collect();
        assert_eq!(
            pats,
            [
                ("/blog".into(), true),
                ("/blog/hello".into(), true),
                ("/notes".into(), true)
            ]
        );
        touch(&root, "blog/hello/+page.wisp");
        assert!(scan(&root).unwrap_err().contains("match the same URLs"));
        touch(&root, "blog/+page.wisp");
        assert!(
            scan(&root)
                .unwrap_err()
                .contains("+page.md and +page.wisp are one page")
        );
        fs::remove_dir_all(&root).unwrap();
        let root = tmp("md-name");
        touch(&root, "[x].md");
        assert!(scan(&root).unwrap_err().contains("Markdown page's name"));
        fs::remove_dir_all(&root).unwrap();

        // Editors' files beside the real ones are not route files.
        let root = tmp("editor");
        for f in [
            "+page.wisp",
            "+page.wisp~",
            ".+page.wisp.swp",
            "#+page.wisp#",
            "4913",
        ] {
            touch(&root, f);
        }
        assert_eq!(scan(&root).unwrap().routes.len(), 1);
        fs::remove_dir_all(&root).unwrap();
    }
}
