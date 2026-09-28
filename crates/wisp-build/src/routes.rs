//! Discovers routes from the `src/routes` directory tree.

use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Seg {
    Static(String),
    Param(String),
    /// `[[name]]`: present or absent.
    Optional(String),
    /// `[...name]`: zero or more segments.
    Rest(String),
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
    pub page_rs: bool,
    pub server: bool,
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
                Seg::Param(n) => s.push_str(&format!("[{n}]")),
                Seg::Optional(n) => s.push_str(&format!("[[{n}]]")),
                Seg::Rest(n) => s.push_str(&format!("[...{n}]")),
            }
        }
        s
    }

    pub fn params(&self) -> Vec<&str> {
        self.segs
            .iter()
            .filter_map(|s| match s {
                Seg::Static(_) => None,
                Seg::Param(n) | Seg::Optional(n) | Seg::Rest(n) => Some(n.as_str()),
            })
            .collect()
    }

    /// Every concrete pattern this route matches: optional segments expand
    /// into present/absent variants.
    pub fn expansions(&self) -> Vec<Vec<&Seg>> {
        let mut out: Vec<Vec<&Seg>> = vec![Vec::new()];
        for seg in &self.segs {
            if let Seg::Optional(_) = seg {
                let with: Vec<_> = out.iter().map(|v| v.iter().copied().chain([seg]).collect()).collect();
                out.extend(with);
            } else {
                out.iter_mut().for_each(|v| v.push(seg));
            }
        }
        out
    }
}

pub const MAX_PARAMS: usize = 8;

pub fn scan(routes_dir: &Path) -> Result<Tree, String> {
    let mut tree = Tree::default();
    if routes_dir.is_dir() {
        walk(routes_dir, &mut Vec::new(), &mut Vec::new(), None, &mut tree)?;
    }

    for r in &tree.routes {
        let params = r.params();
        if params.len() > MAX_PARAMS {
            return Err(format!("{}: more than {MAX_PARAMS} parameters", r.pattern()));
        }
        if r.segs.iter().filter(|s| matches!(s, Seg::Rest(_))).count() > 1 {
            return Err(format!("{}: at most one [...rest] segment per route", r.pattern()));
        }
        for (i, p) in params.iter().enumerate() {
            if params[..i].contains(p) {
                return Err(format!("{}: parameter `{p}` appears twice", r.pattern()));
            }
        }
    }

    // Route order only makes ids and generated code deterministic; matching
    // order comes from `arms`.
    tree.routes.sort_by(|a, b| priority(&a.segs.iter().collect::<Vec<_>>(), &b.segs.iter().collect::<Vec<_>>()));

    // Two routes that can match the exact same URLs are ambiguous.
    let mut seen: Vec<(Vec<String>, usize)> = Vec::new();
    for (id, r) in tree.routes.iter().enumerate() {
        for exp in r.expansions() {
            let key: Vec<String> = exp
                .iter()
                .map(|s| match s {
                    Seg::Static(n) => format!("s:{n}"),
                    Seg::Param(_) | Seg::Optional(_) => "p".into(),
                    Seg::Rest(_) => "r".into(),
                })
                .collect();
            match seen.iter().find(|(k, _)| *k == key) {
                Some(&(_, other)) if other != id => {
                    return Err(format!(
                        "routes {} ({}) and {} ({}) match the same URLs",
                        tree.routes[other].pattern(),
                        tree.routes[other].dir.display(),
                        r.pattern(),
                        r.dir.display()
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

fn walk(dir: &Path, segs: &mut Vec<Seg>, layouts: &mut Vec<usize>, error: Option<usize>, tree: &mut Tree) -> Result<(), String> {
    let mut files = Vec::new();
    let mut dirs = Vec::new();
    for e in fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))? {
        let e = e.map_err(|e| format!("{}: {e}", dir.display()))?;
        let name = e.file_name().to_string_lossy().into_owned();
        if e.file_type().map_err(|e| e.to_string())?.is_dir() {
            dirs.push(name);
        } else if name.starts_with('+') {
            files.push(name);
        }
    }
    files.sort();
    dirs.sort();

    let has = |f: &str| files.iter().any(|x| x == f);
    for f in &files {
        const KNOWN: [&str; 6] = ["+page.wisp", "+page.rs", "+layout.wisp", "+layout.rs", "+error.wisp", "+server.rs"];
        if !KNOWN.contains(&f.as_str()) {
            return Err(format!("{}: unknown route file (expected one of {})", dir.join(f).display(), KNOWN.join(", ")));
        }
    }
    if has("+page.rs") && !has("+page.wisp") {
        return Err(format!("{}: +page.rs needs a +page.wisp next to it", dir.display()));
    }
    if has("+layout.rs") && !has("+layout.wisp") {
        return Err(format!("{}: +layout.rs needs a +layout.wisp next to it", dir.display()));
    }

    let depth = layouts.len();
    if has("+layout.wisp") {
        tree.layouts.push(Layout { dir: dir.to_path_buf(), has_rs: has("+layout.rs") });
        layouts.push(tree.layouts.len() - 1);
    }
    let mut error = error;
    if has("+error.wisp") {
        tree.errors.push(ErrorPage { dir: dir.to_path_buf(), layouts: layouts.clone() });
        error = Some(tree.errors.len() - 1);
    }
    if has("+page.wisp") || has("+server.rs") {
        tree.routes.push(Route {
            dir: dir.to_path_buf(),
            segs: segs.clone(),
            page: has("+page.wisp"),
            page_rs: has("+page.rs"),
            server: has("+server.rs"),
            layouts: layouts.clone(),
            error,
        });
    }

    for d in dirs {
        let seg = parse_segment(&d).map_err(|e| format!("{}: {e}", dir.join(&d).display()))?;
        let pushed = seg.is_some();
        if let Some(s) = seg {
            segs.push(s);
        }
        walk(&dir.join(&d), segs, layouts, error, tree)?;
        if pushed {
            segs.pop();
        }
    }
    layouts.truncate(depth);
    Ok(())
}

/// `None` for `(group)` directories, which do not appear in the URL.
fn parse_segment(name: &str) -> Result<Option<Seg>, String> {
    let ident = |s: &str| -> Result<String, String> {
        if s.contains('=') {
            return Err("param matchers ([name=matcher]) are not supported yet".into());
        }
        let ok = !s.is_empty()
            && s.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_')
            && !s.as_bytes()[0].is_ascii_digit();
        if !ok {
            return Err(format!("`{s}` is not a valid parameter name"));
        }
        Ok(s.to_string())
    };
    if name.starts_with('(') && name.ends_with(')') {
        return Ok(None);
    }
    if let Some(inner) = name.strip_prefix("[[").and_then(|s| s.strip_suffix("]]")) {
        return Ok(Some(Seg::Optional(ident(inner)?)));
    }
    if let Some(inner) = name.strip_prefix("[...").and_then(|s| s.strip_suffix(']')) {
        return Ok(Some(Seg::Rest(ident(inner)?)));
    }
    if let Some(inner) = name.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
        return Ok(Some(Seg::Param(ident(inner)?)));
    }
    if name.contains(['[', ']', '(', ')']) {
        return Err("mixed static and dynamic text in one segment is not supported".into());
    }
    Ok(Some(Seg::Static(name.to_string())))
}

/// Static beats param beats optional beats rest, left to right. When one
/// pattern is a prefix of the other, the shorter (more exact) one goes first.
fn priority(a: &[&Seg], b: &[&Seg]) -> std::cmp::Ordering {
    let rank = |s: &Seg| match s {
        Seg::Static(_) => 0,
        Seg::Param(_) => 1,
        Seg::Optional(_) => 2,
        Seg::Rest(_) => 3,
    };
    for (&x, &y) in a.iter().zip(b) {
        let ord = rank(x).cmp(&rank(y)).then_with(|| match (x, y) {
            (Seg::Static(p), Seg::Static(q)) => p.cmp(q),
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
        let arms: Vec<_> = t.arms().iter().map(|(_, id)| t.routes[*id].pattern()).collect();
        let hello = arms.iter().position(|p| p == "/[[lang]]/hello").unwrap();
        let slug = arms.iter().position(|p| p == "/[slug]").unwrap();
        assert!(hello < slug, "{arms:?}");
        fs::remove_dir_all(&root).unwrap();
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
        assert!(scan(&root).unwrap_err().contains("match the same URLs"));
        fs::remove_dir_all(&root).unwrap();
    }
}
