//! `wisp routes` lists what the app serves; `wisp new-route` writes a route.

use std::fs;
use std::path::Path;
use wisp_build::routes::{self, HANDLERS, Route, Seg};
use wisp_build::rust_scan;

use crate::term;

/// Methods, URL pattern and file of a route.
type Row = (String, String, String);

/// `wisp routes`: each method, URL pattern and file, in match order.
pub fn list(root: &Path, args: &[String]) -> Result<(), String> {
    if let Some(arg) = args.first() {
        return Err(format!("Unexpected {arg}.\nwisp routes takes no options."));
    }
    let rows = rows(root)?;
    if rows.is_empty() {
        println!("The app has no routes yet. Add one with wisp new-route /about.");
    }
    let m = rows.iter().map(|r| r.0.len()).max().unwrap_or(0);
    let p = rows.iter().map(|r| r.1.len()).max().unwrap_or(0);
    for (methods, pattern, file) in &rows {
        println!(
            "{}  {pattern:p$}  {}",
            term::accent(&format!("{methods:m$}")),
            term::dim(file)
        );
    }
    Ok(())
}

/// Every route's rows. A directory with a page and a `+server.rs` gives
/// each its own.
fn rows(root: &Path) -> Result<Vec<Row>, String> {
    let tree = routes::scan(&root.join("src/routes"))?;
    let file = |r: &Route, name: &str| {
        let path = r.dir.join(name);
        let path = path.strip_prefix(root).unwrap_or(&path);
        path.to_string_lossy().replace('\\', "/")
    };
    let mut rows = Vec::new();
    for r in &tree.routes {
        let pattern = r.pattern();
        if r.page {
            rows.push(("GET".into(), pattern.clone(), file(r, "+page.wisp")));
        }
        if r.server {
            let path = r.dir.join("+server.rs");
            let src = fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
            let items = rust_scan::scan(&src).map_err(|e| format!("{}: {e}", path.display()))?;
            let (own, member) = methods(&items, &r.segs);
            let shown = file(r, "+server.rs");
            if !own.is_empty() {
                rows.push((own, pattern.clone(), shown.clone()));
            }
            if !member.is_empty() {
                let at = if pattern == "/" { "" } else { &pattern };
                rows.push((member, format!("{at}/[id]"), shown));
            }
        }
    }
    Ok(rows)
}

/// The methods a `+server.rs` answers at its route and at its `/[id]`.
fn methods(items: &rust_scan::Items, segs: &[Seg]) -> (String, String) {
    const NAMES: [&str; 5] = ["GET", "POST", "PUT", "PATCH", "DELETE"];
    let slot = |name: &str| match name {
        "get" | "list" => 0,
        "post" => 1,
        "put" => 2,
        "patch" => 3,
        _ => 4,
    };
    let (mut own, mut member) = ([false; 5], [false; 5]);
    for f in items
        .fns
        .iter()
        .filter(|f| HANDLERS.contains(&f.name.as_str()))
    {
        let set = if routes::is_member(f, segs) {
            &mut member
        } else {
            &mut own
        };
        set[slot(&f.name)] = true;
    }
    if routes::rest_type(items).is_some() {
        own[..2].fill(true);
        member = [true, false, true, true, true];
    }
    let join = |set: [bool; 5]| {
        let on: Vec<&str> = (0..5).filter(|&i| set[i]).map(|i| NAMES[i]).collect();
        on.join(",")
    };
    (join(own), join(member))
}

/// `wisp new-route <path> [page|server|rest]`.
pub fn new_route(root: &Path, args: &[String]) -> Result<(), String> {
    let usage = "Usage: wisp new-route <path> [page|server|rest], such as wisp new-route /blog/[slug] page.";
    let (path, kind) = match args {
        [path] => (path, "page"),
        [path, kind] => (path, kind.as_str()),
        _ => return Err(usage.into()),
    };
    if !matches!(kind, "page" | "server" | "rest") {
        return Err(format!("There is no kind {kind}.\n{usage}"));
    }
    let made = create(root, path, kind)?;
    term::done(&format!("Wrote {made}"));
    Ok(())
}

/// Writes the route's file and returns where; a file that exists is never
/// replaced.
fn create(root: &Path, path: &str, kind: &str) -> Result<String, String> {
    let segs: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    for s in &segs {
        let plain = s
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_.[]=()".contains(c));
        if !plain || s.starts_with('.') {
            return Err(format!(
                "{s} is not a folder name for a route.\nUse letters, digits, - and _, or [param], [[optional]], [...rest], (group)."
            ));
        }
    }
    let name = segs
        .iter()
        .rev()
        .find(|s| !s.starts_with(['[', '(']))
        .copied()
        .unwrap_or("");
    let (file, text) = match kind {
        "page" => {
            let title = if name.is_empty() {
                "Home".into()
            } else {
                camel(name, " ")
            };
            (
                "+page.wisp",
                format!("<title>{title}</title>\n\n<h1>{title}</h1>\n"),
            )
        }
        "server" => (
            "+server.rs",
            "fn get() -> &'static str {\n    \"ok\"\n}\n".into(),
        ),
        _ => {
            let one = name
                .strip_suffix('s')
                .filter(|n| n.len() > 2 && !n.ends_with('s'));
            let ty = camel(one.unwrap_or(name), "");
            let ty = if ty.is_empty() { "Item".into() } else { ty };
            (
                "+server.rs",
                format!(
                    "#[derive(Rest)]\nstruct {ty} {{\n    #[validate(len = 1..=200)]\n    title: String,\n}}\n"
                ),
            )
        }
    };
    let dir = segs.iter().fold(root.join("src/routes"), |d, s| d.join(s));
    // A page and a server for one URL would both answer GET.
    let other = if kind == "page" {
        "+server.rs"
    } else {
        "+page.wisp"
    };
    if let Some(taken) = [file, other].iter().find(|f| dir.join(f).exists()) {
        let shown = dir.join(taken);
        return Err(format!(
            "{} already exists.\nWisp does not replace a file; edit it, or pick another path.",
            shown
                .strip_prefix(root)
                .unwrap_or(&shown)
                .to_string_lossy()
                .replace('\\', "/")
        ));
    }
    fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let to = dir.join(file);
    fs::write(&to, text).map_err(|e| format!("{}: {e}", to.display()))?;
    Ok(to
        .strip_prefix(root)
        .unwrap_or(&to)
        .to_string_lossy()
        .replace('\\', "/"))
}

/// `blog-posts` as `Blog Posts` or `BlogPosts`, by what goes between.
fn camel(name: &str, between: &str) -> String {
    let words: Vec<String> = name
        .split(['-', '_'])
        .filter(|w| !w.is_empty())
        .map(|w| {
            let mut c = w.chars();
            c.next()
                .map(|f| f.to_uppercase().chain(c).collect())
                .unwrap_or_default()
        })
        .collect();
    words.join(between)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("wisp-routes-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn new_routes_are_listed() {
        let root = temp("list");
        create(&root, "/", "page").unwrap();
        create(&root, "/blog/[slug]", "page").unwrap();
        create(&root, "/api/notes", "rest").unwrap();
        create(&root, "healthz", "server").unwrap();
        let text = fs::read_to_string(root.join("src/routes/api/notes/+server.rs")).unwrap();
        assert!(text.contains("struct Note {"), "{text}");
        let got = rows(&root).unwrap();
        let want = |m: &str, p: &str, f: &str| (m.to_string(), p.to_string(), f.to_string());
        for row in [
            want("GET", "/", "src/routes/+page.wisp"),
            want("GET", "/blog/[slug]", "src/routes/blog/[slug]/+page.wisp"),
            want("GET,POST", "/api/notes", "src/routes/api/notes/+server.rs"),
            want(
                "GET,PUT,PATCH,DELETE",
                "/api/notes/[id]",
                "src/routes/api/notes/+server.rs",
            ),
            want("GET", "/healthz", "src/routes/healthz/+server.rs"),
        ] {
            assert!(got.contains(&row), "{row:?} in {got:?}");
        }
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn it_never_replaces_or_escapes() {
        let root = temp("refuse");
        create(&root, "/about", "page").unwrap();
        let again = create(&root, "/about", "page").unwrap_err();
        assert!(
            again.contains("src/routes/about/+page.wisp already exists"),
            "{again}"
        );
        assert!(create(&root, "/about", "server").is_err());
        assert!(create(&root, "/../x", "page").is_err());
        assert!(create(&root, "/a b", "page").is_err());
        let page = fs::read_to_string(root.join("src/routes/about/+page.wisp")).unwrap();
        assert_eq!(page, "<title>About</title>\n\n<h1>About</h1>\n");
        let _ = fs::remove_dir_all(&root);
    }
}
