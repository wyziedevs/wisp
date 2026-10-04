//! Per-route runtime: a route with `const RUNTIME: wisp::Runtime =
//! wisp::Runtime::Edge;` runs in Vercel's or Netlify's edge function, the rest
//! in their Node one. The routes the edge answers, as URL patterns for each
//! host's routing, and the check that they can run there.

use std::path::Path;
use wisp_build::routes::{self, Seg};
use wisp_build::rust_scan;

/// What the wasm build has no way to do, and what it is.
const NO: [(&str, &str); 8] = [
    ("std::fs", "files"),
    ("tokio::fs", "files"),
    ("std::thread", "threads"),
    ("std::process", "processes"),
    ("tokio::process", "processes"),
    ("std::net", "sockets"),
    ("tokio::net", "sockets"),
    (".websocket(", "websockets (use SSE)"),
];

/// The edge routes' patterns, each a list of segments (`[[x]]` expanded, a
/// `+server.rs`'s `/[id]` added). Empty when no route is Edge.
pub fn edge_routes(root: &Path) -> Result<Vec<Vec<Seg>>, String> {
    let tree = routes::scan(&root.join("src/routes"))?;
    let mut out = Vec::new();
    for r in &tree.routes {
        let (mut edge, mut at) = (None, String::new());
        for (name, here) in [("+page.rs", r.page_rs), ("+server.rs", r.server)] {
            if !here {
                continue;
            }
            let path = r.dir.join(name);
            let src =
                std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
            let items = rust_scan::scan(&src).map_err(|e| format!("{}: {e}", path.display()))?;
            let rel = path
                .strip_prefix(root)
                .unwrap_or(&path)
                .display()
                .to_string()
                .replace('\\', "/");
            let this = routes::edge(&items).map_err(|(line, m)| format!("{rel}:{line}: {m}"))?;
            if this.is_some() && edge.is_some() && this != edge {
                return Err(format!(
                    "{rel}: route {} is set in {at} and here to different runtimes; a route runs in one place.",
                    r.pattern()
                ));
            }
            if this == Some(true) {
                forbidden(&src, &r.pattern(), &rel)?;
            }
            if this.is_some() && edge.is_none() {
                (edge, at) = (this, rel);
            }
        }
        if edge == Some(true) {
            for e in r.expansions() {
                let mut segs: Vec<Seg> = e.into_iter().cloned().collect();
                out.push(segs.clone());
                if r.member {
                    segs.push(Seg::Param("id".into(), None));
                    out.push(segs);
                }
            }
        }
    }
    Ok(out)
}

/// An Edge route's source must not use what wasm lacks.
fn forbidden(src: &str, pattern: &str, file: &str) -> Result<(), String> {
    for (n, line) in src.lines().enumerate() {
        let code = line.split("//").next().unwrap_or("");
        if let Some((used, what)) = NO.iter().find(|(u, _)| code.contains(u)) {
            let used = used.trim_matches(['.', '(']);
            return Err(format!(
                "{file}:{}: route {pattern} is `RUNTIME = Edge`, which has no {what}, and this uses `{used}`.\nDrop `RUNTIME` to run it on Node, or take `{used}` out.",
                n + 1
            ));
        }
    }
    Ok(())
}

/// Vercel's `src` regex for a pattern.
pub fn vercel(segs: &[Seg]) -> String {
    let mut s = String::from("^");
    for seg in segs {
        match seg {
            Seg::Static(n) => {
                s.push('/');
                for c in n.chars() {
                    if !c.is_alphanumeric() && !"-_~".contains(c) {
                        s.push('\\');
                    }
                    s.push(c);
                }
            }
            Seg::Rest(_) => s += "(?:/.*)?",
            _ => s += "/[^/]+",
        }
    }
    if segs.is_empty() {
        s.push('/');
    }
    s + "/?$"
}

/// Netlify's `path` (URLPattern) for a pattern.
pub fn netlify(segs: &[Seg]) -> String {
    let mut s = String::new();
    for seg in segs {
        match seg {
            Seg::Static(n) => s += &format!("/{n}"),
            Seg::Rest(_) => s += "/*",
            Seg::Param(n, _) | Seg::Optional(n, _) => s += &format!("/:{n}"),
        }
    }
    if s.is_empty() { "/".into() } else { s }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app(name: &str, files: &[(&str, &str)]) -> std::path::PathBuf {
        let root = std::env::temp_dir().join(format!("wisp-runtime-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for (p, src) in files {
            let f = root.join(p);
            std::fs::create_dir_all(f.parent().unwrap()).unwrap();
            std::fs::write(f, src).unwrap();
        }
        root
    }

    const EDGE: &str = "const RUNTIME: wisp::Runtime = wisp::Runtime::Edge;\n";

    #[test]
    fn edge_routes_become_patterns() {
        let root = app(
            "ok",
            &[
                ("src/routes/+page.wisp", "x"),
                ("src/routes/blog/[slug]/+page.wisp", "x"),
                ("src/routes/blog/[slug]/+page.rs", EDGE),
                ("src/routes/docs/[...p]/+page.wisp", "x"),
                (
                    "src/routes/docs/[...p]/+page.rs",
                    "const RUNTIME: wisp::Runtime = wisp::Runtime::Node;",
                ),
                (
                    "src/routes/hi.json/+server.rs",
                    &format!("{EDGE}pub async fn get() {{}}"),
                ),
            ],
        );
        let routes = edge_routes(&root).unwrap();
        let v: Vec<_> = routes.iter().map(|s| (vercel(s), netlify(s))).collect();
        assert!(
            v.contains(&("^/blog/[^/]+/?$".into(), "/blog/:slug".into())),
            "{v:?}"
        );
        assert!(
            v.contains(&("^/hi\\.json/?$".into(), "/hi.json".into())),
            "{v:?}"
        );
        assert_eq!(v.len(), 2, "{v:?}");
        assert_eq!(vercel(&[Seg::Rest("p".into())]), "^(?:/.*)?/?$");
        assert_eq!(vercel(&[]), "^//?$");
        assert_eq!(
            netlify(&[Seg::Static("a".into()), Seg::Rest("p".into())]),
            "/a/*"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn no_edge_route_no_patterns() {
        let root = app(
            "none",
            &[
                ("src/routes/+page.wisp", "x"),
                ("src/routes/+page.rs", "const A: u8 = 1;"),
            ],
        );
        assert!(edge_routes(&root).unwrap().is_empty());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn an_edge_route_that_needs_files_is_an_error() {
        let src = format!("{EDGE}fn load() {{ let _ = std::fs::read(\"x\"); }}\n");
        let root = app(
            "fs",
            &[
                ("src/routes/a/+page.wisp", "x"),
                ("src/routes/a/+page.rs", &src),
            ],
        );
        let e = edge_routes(&root).unwrap_err();
        assert!(e.contains("src/routes/a/+page.rs:2"), "{e}");
        assert!(
            e.contains("route /a") && e.contains("no files") && e.contains("std::fs"),
            "{e}"
        );
        let _ = std::fs::remove_dir_all(root);
        let root = app(
            "lit",
            &[
                ("src/routes/+page.wisp", "x"),
                (
                    "src/routes/+page.rs",
                    "const RUNTIME: wisp::Runtime = pick();",
                ),
            ],
        );
        assert!(edge_routes(&root).unwrap_err().contains("as a literal"));
        let _ = std::fs::remove_dir_all(root);
    }
}
