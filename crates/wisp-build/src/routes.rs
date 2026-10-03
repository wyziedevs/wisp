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

#[derive(Debug, Default)]
pub struct Tree {
    /// Sorted by match priority; a route's id is its index.
    pub routes: Vec<Route>,
    pub layouts: Vec<Layout>,
    pub errors: Vec<ErrorPage>,
    /// The param matchers routes use: each name with its `src/params` file,
    /// or `None` for the built-in `int`.
    pub matchers: Vec<(String, Option<PathBuf>)>,
}

/// `=int` for a matcher, nothing without one.
fn matcher(m: &Option<String>) -> String {
    m.as_ref().map_or(String::new(), |m| format!("={m}"))
}

impl Route {
    /// The route as a URL pattern, for messages: `/blog/[slug]`.
    pub fn pattern(&self) -> String {
        if self.segs.is_empty() {
            return "/".into();
        }
        let mut s = String::new();
        for seg in &self.segs {
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
        let name = e.file_name().to_string_lossy().into_owned();
        if editor_temp(&name) {
            continue;
        }
        if e.file_type().map_err(|e| e.to_string())?.is_dir() {
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
        const KNOWN: [&str; 9] = [
            "+page.wisp",
            "+page.rs",
            "+page.js",
            "+page.ts",
            "+page.md",
            "+layout.wisp",
            "+layout.rs",
            "+error.wisp",
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
    let mut error = error;
    if has("+error.wisp") {
        tree.errors.push(ErrorPage {
            dir: dir.to_path_buf(),
            layouts: layouts.clone(),
        });
        error = Some(tree.errors.len() - 1);
    }
    let (collection, member) = match has("+server.rs") {
        true => server_shape(&dir.join("+server.rs"), segs),
        false => (false, None),
    };
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
    if has("+page.wisp") || page_md.is_some() || collection {
        tree.routes.push(Route {
            dir: dir.to_path_buf(),
            segs: segs.clone(),
            page: has("+page.wisp") || page_md.is_some(),
            page_file,
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
            page_file: String::new(),
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

/// What a `+server.rs` serves: its route (true unless every handler is
/// its `/[id]`'s), and that `/[id]`, if any, with the matcher `int` when
/// every `id` it takes is an integer. A file that does not scan is left to
/// `codegen`, which says what is wrong.
fn server_shape(file: &Path, segs: &[Seg]) -> (bool, Option<Option<String>>) {
    let Some(items) = crate::read_source(file)
        .ok()
        .and_then(|s| crate::rust_scan::scan(&s).ok())
    else {
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
    let ident = |s: &str| -> Result<String, String> {
        if !crate::ty::is_ident(s) {
            return Err(format!("`{s}` is not a valid parameter name"));
        }
        Ok(s.to_string())
    };
    // `name=matcher`: the matcher is a file in src/params, or `int`.
    let param = |s: &str| -> Result<(String, Option<String>), String> {
        match s.split_once('=') {
            Some((n, m)) => {
                let m = ident(m).map_err(|_| format!("`{m}` is not a valid matcher name"))?;
                Ok((ident(n)?, Some(m)))
            }
            None => Ok((ident(s)?, None)),
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
        return Ok(Some(Seg::Rest(ident(inner)?)));
    }
    if let Some(inner) = name.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
        let (n, m) = param(inner)?;
        return Ok(Some(Seg::Param(n, m)));
    }
    if name.contains(['[', ']', '(', ')']) {
        return Err("mixed static and dynamic text in one segment is not supported".into());
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
