//! What an app is made of, for tools (`wisp mcp`): its routes with their
//! params, actions and endpoints, and its components with their props.
//! Read from the files as the build reads them; nothing is compiled.

use crate::routes::{self, HANDLERS};
use crate::rust_scan::{self, Items};
use crate::template::PropDecl;
use std::path::Path;

/// A route: `/blog/[slug]`, its params, the `#[action]`s of its page and
/// what its `+server.rs` answers (`GET /api/notes/[id]`).
pub struct RouteInfo {
    pub pattern: String,
    /// The folder, from the app's root: `src/routes/blog/[slug]`.
    pub dir: String,
    pub params: Vec<String>,
    pub page: bool,
    pub actions: Vec<String>,
    pub endpoints: Vec<String>,
}

pub fn routes(root: &Path) -> Result<Vec<RouteInfo>, String> {
    let tree = routes::scan(&root.join("src").join("routes"))?;
    let rel = |p: &Path| {
        let p = p.strip_prefix(root).unwrap_or(p);
        p.to_string_lossy().replace('\\', "/")
    };
    let mut out = Vec::new();
    for r in &tree.routes {
        let pattern = r.pattern();
        let page = (r.page || r.page_rs).then(|| page_items(&r.dir));
        let actions = page
            .iter()
            .flat_map(|i| i.fns.iter().filter(|f| f.action))
            .map(|f| f.name.clone())
            .collect();
        let mut endpoints = Vec::new();
        if r.server {
            let items = crate::read_source(&r.dir.join("+server.rs"))
                .ok()
                .and_then(|s| rust_scan::scan(&s).ok())
                .unwrap_or_default();
            let base = pattern.trim_end_matches('/');
            let at = |member: bool| {
                if member {
                    format!("{base}/[id]")
                } else {
                    pattern.clone()
                }
            };
            if routes::rest_type(&items).is_some() && !r.params().contains(&"id") {
                for (m, member) in [
                    ("GET", false),
                    ("POST", false),
                    ("GET", true),
                    ("PUT", true),
                    ("PATCH", true),
                    ("DELETE", true),
                ] {
                    endpoints.push(format!("{m} {}", at(member)));
                }
            }
            for f in items
                .fns
                .iter()
                .filter(|f| HANDLERS.contains(&f.name.as_str()))
            {
                let method = match f.name.as_str() {
                    "list" => "GET".to_string(),
                    m => m.to_ascii_uppercase(),
                };
                let e = format!("{method} {}", at(routes::is_member(f, &r.segs)));
                if !endpoints.contains(&e) {
                    endpoints.push(e);
                }
            }
        }
        out.push(RouteInfo {
            dir: rel(&r.dir),
            params: r.params().iter().map(|p| p.to_string()).collect(),
            page: r.page,
            pattern,
            actions,
            endpoints,
        });
    }
    Ok(out)
}

/// The functions of a page's block, or of its `+page.rs`. What does not
/// scan has none: the build says what is wrong.
fn page_items(dir: &Path) -> Items {
    let block = crate::read_source(&dir.join("+page.wisp"))
        .ok()
        .and_then(|src| crate::split_front(&src).ok()?.0);
    let src = match block {
        Some(b) => Some(rust_scan::split_items(&b).0),
        None => crate::read_source(&dir.join("+page.rs")).ok(),
    };
    src.and_then(|s| rust_scan::scan(&s).ok())
        .unwrap_or_default()
}

/// `src/components/**/*.wisp`: each component's name, file and props.
pub fn components(root: &Path) -> Result<Vec<(String, String, Vec<PropDecl>)>, String> {
    let dir = root.join("src").join("components");
    let mut files = Vec::new();
    if dir.is_dir() {
        crate::codegen::list_files(&dir, &mut files)?;
    }
    files.sort();
    let mut out = Vec::new();
    for file in files
        .iter()
        .filter(|f| f.extension().is_some_and(|e| e == "wisp"))
    {
        let rel = file.strip_prefix(root).unwrap_or(file);
        let rel = rel.to_string_lossy().replace('\\', "/");
        let src = crate::read_source(file).map_err(|e| format!("{rel}: {e}"))?;
        let (t, _) = crate::parse_wisp(&src).map_err(|e| format!("{rel}:{e}"))?;
        let name = file.file_stem().unwrap_or_default().to_string_lossy();
        out.push((
            name.into_owned(),
            rel,
            t.props.map(|p| p.0).unwrap_or_default(),
        ));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use std::fs;

    #[test]
    fn routes_and_components() {
        let root = std::env::temp_dir().join(format!("wisp-inspect-{}", std::process::id()));
        let write = |rel: &str, text: &str| {
            let p = root.join(rel);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(p, text).unwrap();
        };
        write(
            "src/routes/blog/[slug]/+page.wisp",
            "---\n#[action]\nfn like() {}\nlet n = 1;\n---\n<p>{slug}</p>",
        );
        write(
            "src/routes/api/notes/+server.rs",
            "#[derive(Rest)]\nstruct Note { title: String }\nfn post(body: Note) {}\n",
        );
        write(
            "src/routes/api/tags/+server.rs",
            "fn list() -> Vec<u8> { vec![] }\nfn delete(id: u64) {}\n",
        );
        write(
            "src/components/Card.wisp",
            "{@props title: &str, n: u32 = 0}\n<p>{title}</p>",
        );
        let routes = super::routes(&root).unwrap();
        let find = |p: &str| routes.iter().find(|r| r.pattern == p).unwrap();
        let blog = find("/blog/[slug]");
        assert_eq!(
            (blog.params.clone(), blog.actions.clone()),
            (vec!["slug".into()], vec!["like".into()])
        );
        assert_eq!(blog.dir, "src/routes/blog/[slug]");
        let notes = &find("/api/notes").endpoints;
        assert_eq!(notes.len(), 6);
        assert!(notes.contains(&"PATCH /api/notes/[id]".to_string()));
        assert_eq!(
            find("/api/tags").endpoints,
            ["GET /api/tags", "DELETE /api/tags/[id]"]
        );
        let comps = super::components(&root).unwrap();
        assert_eq!(comps[0].0, "Card");
        assert_eq!(comps[0].2[1].default.as_deref(), Some("0"));
        fs::remove_dir_all(&root).unwrap();
    }
}
