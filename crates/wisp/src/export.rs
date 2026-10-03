//! Static export, for `wisp build --static`: every page that needs no
//! server, rendered through [`crate::handle`] and written as files.
//!
//! The app's own binary does it: `WISP_EXPORT=dist` makes [`crate::run`]
//! write the site instead of serving it. It prints one line per file
//! (`wrote about/index.html`) and per problem (`warn ...`), which the CLI
//! shows.
//!
//! A route with parameters is exported once for each entry of its
//! `pub fn entries() -> Vec<...>` in `+page.rs`: a `String` per `[param]`,
//! or a tuple of them for several, in the order they appear in the path.
//! For `[[optional]]` and `[...rest]`, an empty string leaves it out.
//!
//! `wisp build` runs it with `WISP_PRERENDER=dir` too, when a page has
//! `const PRERENDER: bool = true;`: [`prerender`] writes those pages for
//! the build that follows to embed.
//!
//! `wisp build --spa` (`WISP_SPA=1`) also serves static hosts' fallback,
//! `index.html`, which they answer a path they have no file for with: a
//! page the browser draws (`const SSR: bool = false;`) whose route has
//! parameters and no `entries` is written once, each parameter `0`, to
//! `_app/spa/N.html`, and `index.html` lists those as `#wisp-spa`. wisp.js
//! then fetches the one whose route fits the address, and draws it with
//! that address's parameters.

use crate::{App, Request, handle};
use std::collections::BTreeSet;
use std::fs;
use std::io;
use std::path::Path;

/// A route as the export sees it. Made by the generated code.
pub struct ExportRoute {
    /// `/blog/[slug]`.
    pub pattern: &'static str,
    pub page: bool,
    /// The page has actions, which need a server.
    pub actions: bool,
    /// There is a `+server.rs`.
    pub server: bool,
    /// The values of the route's parameters for each page, from `entries`.
    pub entries: Option<fn() -> Vec<Vec<String>>>,
    /// `/sitemap.xml` lists it: outside any `(private)` group, without a
    /// robots `noindex` meta.
    pub indexed: bool,
    /// The server renders the page: no `const SSR: bool = false;`.
    pub ssr: bool,
    /// `const PRERENDER: bool = true;`: `wisp build` renders it.
    pub prerender: bool,
}

/// What `entries` returns a `Vec` of.
pub trait Entry {
    fn params(self) -> Vec<String>;
}

impl Entry for String {
    fn params(self) -> Vec<String> {
        vec![self]
    }
}

impl Entry for &str {
    fn params(self) -> Vec<String> {
        vec![self.into()]
    }
}

impl<A: Into<String>, B: Into<String>> Entry for (A, B) {
    fn params(self) -> Vec<String> {
        vec![self.0.into(), self.1.into()]
    }
}

impl<A: Into<String>, B: Into<String>, C: Into<String>> Entry for (A, B, C) {
    fn params(self) -> Vec<String> {
        vec![self.0.into(), self.1.into(), self.2.into()]
    }
}

/// `job` on a runtime of this thread's, as `wisp build` runs the app.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn run(job: impl std::future::Future<Output = io::Result<()>>) -> io::Result<()> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    runtime.block_on(job)
}

/// For `wisp build`: the pages with `const PRERENDER: bool = true;`, each
/// rendered once into `dir` as `N.html`, and `index.tsv` a line for each:
/// its route's pattern, its path and its file, tab apart. The build that
/// follows embeds them (see `wisp_build`'s `prerendered`).
pub async fn prerender<A: App>(dir: &Path) -> io::Result<()> {
    crate::prepare::<A>().await?;
    fs::create_dir_all(dir)?;
    let mut index = String::new();
    for r in A::export_routes().into_iter().filter(|r| r.prerender) {
        let paths = match paths(&r) {
            Ok(p) => p,
            Err(e) => {
                println!("warn {e}");
                continue;
            }
        };
        for segs in paths {
            let url = url(&segs);
            let reply = handle::<A>(page(&url)).await;
            if reply.status != 200 {
                println!(
                    "warn {url} answered {}, so it is not prerendered",
                    reply.status
                );
                continue;
            }
            let file = format!("{}.html", index.lines().count());
            write(dir, &file, reply.bytes())?;
            index.push_str(&format!("{}\t{url}\t{file}\n", r.pattern));
        }
    }
    fs::write(dir.join("index.tsv"), index)
}

/// Writes the app's pages, and the files they use, under `dir`; with
/// `spa`, the fallback too (see the module's notes).
pub async fn export<A: App>(dir: &Path, spa: bool) -> io::Result<()> {
    crate::prepare::<A>().await?;
    let mut assets = BTreeSet::new();
    // Pattern and file of each page the fallback draws.
    let mut drawn: Vec<(&str, String)> = Vec::new();
    for r in A::export_routes() {
        if r.server {
            println!(
                "warn {} has a +server.rs, which needs a server, so it is not exported",
                r.pattern
            );
        }
        if r.actions {
            println!(
                "warn {} has actions, which need a server: its forms will not work",
                r.pattern
            );
        }
        if !r.page {
            continue;
        }
        let paths = match paths(&r) {
            Ok(p) => p,
            Err(_) if spa && !r.ssr && r.entries.is_none() => {
                let n = r.pattern.split('/').filter(|s| s.starts_with('[')).count();
                let url = segments(r.pattern, &vec!["0".to_string(); n]).map(|s| url(&s));
                let reply = match url {
                    Some(url) => handle::<A>(page(&url)).await,
                    None => continue,
                };
                if reply.status != 200 {
                    println!(
                        "warn {} answered {} with its parameters 0, so --spa cannot draw it",
                        r.pattern, reply.status
                    );
                    continue;
                }
                let file = format!("_app/spa/{}.html", drawn.len());
                find_assets(reply.text(), &mut assets);
                write(dir, &file, reply.bytes())?;
                drawn.push((r.pattern, format!("/{file}")));
                continue;
            }
            Err(e) => {
                println!("warn {e}");
                continue;
            }
        };
        for segs in paths {
            let url = url(&segs);
            let reply = handle::<A>(page(&url)).await;
            if reply.status != 200 {
                println!(
                    "warn {url} answered {}, so it is not exported",
                    reply.status
                );
                continue;
            }
            find_assets(reply.text(), &mut assets);
            write(dir, &file(&segs), reply.bytes())?;
        }
    }
    // What hosts such as GitHub Pages show for a missing page.
    let missing = handle::<A>(page("/_wisp_missing")).await;
    if missing.status == 404 {
        find_assets(missing.text(), &mut assets);
        write(dir, "404.html", missing.bytes())?;
    }
    // A static host has no request host: the sitemap needs `SITE_URL`.
    if std::env::var_os("SITE_URL").is_some_and(|s| !s.is_empty()) {
        for f in ["sitemap.xml", "robots.txt"] {
            let reply = handle::<A>(page(&format!("/{f}"))).await;
            if reply.status == 200 {
                write(dir, f, reply.bytes())?;
            }
        }
    }
    if !drawn.is_empty() {
        // The home page, or with none the error page, carries the list.
        let html = fs::read_to_string(dir.join("index.html"))
            .unwrap_or_else(|_| missing.text().to_string());
        let list = format!(
            "<script type=\"application/json\" id=\"wisp-spa\">{}</script>",
            crate::json::to_json(&drawn).replace('<', "\\u003c")
        );
        let at = html.rfind("</body>").unwrap_or(html.len());
        write(
            dir,
            "index.html",
            [&html[..at], &list, &html[at..]].concat().as_bytes(),
        )?;
    }
    // The service worker and manifest, and custom elements' modules, which
    // no page names.
    for path in [
        crate::protocol::SERVICE_WORKER_PATH,
        crate::protocol::MANIFEST_PATH,
    ]
    .iter()
    .filter(|_| A::PWA.is_some())
    {
        let reply = handle::<A>(page(path)).await;
        if reply.status == 200 {
            find_assets(reply.text(), &mut assets); // the icons, the files kept
            write(dir, &path[1..], reply.bytes())?;
        }
    }
    for tag in A::ELEMENTS {
        assets.insert(format!("{}{tag}.js", crate::protocol::ELEMENTS));
    }
    // A module brings what it imports, and its source map when it has one
    // (`wisp build --sourcemap`).
    let mut done = BTreeSet::new();
    while let Some(path) = assets.pop_first() {
        if !done.insert(path.clone()) {
            continue;
        }
        let Some(rel) = crate::http::safe_relative_path(&path) else {
            continue;
        };
        let reply = handle::<A>(page(&path)).await;
        if reply.status != 200 {
            continue;
        }
        if !path.ends_with(".js") {
            write(dir, &rel, reply.bytes())?;
            continue;
        }
        let js = reply.text();
        find_assets(js, &mut assets);
        let Some((code, map)) = source_map(js) else {
            write(dir, &rel, reply.bytes())?;
            continue;
        };
        let at = rel.rfind('/').map_or(0, |i| i + 1);
        let found = handle::<A>(page(&format!("/{}{map}", &rel[..at]))).await;
        if found.status == 200 {
            write(dir, &format!("{}{map}", &rel[..at]), found.bytes())?;
            write(dir, &rel, reply.bytes())?;
        } else {
            // No map to point at: the module without the line.
            write(dir, &rel, code.as_bytes())?;
        }
    }
    Ok(())
}

/// A GET as a browser makes it, so an error is a page, as the host shows it.
fn page(target: &str) -> Request {
    let mut req = Request::new("GET", target);
    req.header("accept", "text/html");
    req
}

/// A module's code before its `//# sourceMappingURL=` line, and the map's
/// file beside it that the line names.
fn source_map(js: &str) -> Option<(&str, &str)> {
    let (code, tail) = js.rsplit_once("//# sourceMappingURL=")?;
    let map = tail.lines().next()?.trim();
    let plain = !map.is_empty() && !map.contains([':', '/', '?', '\\']) && !map.starts_with('.');
    plain.then_some((code, map))
}

/// The pages a route makes, as path segments.
pub(crate) fn paths(r: &ExportRoute) -> Result<Vec<Vec<String>>, String> {
    let params: Vec<&str> = r
        .pattern
        .split('/')
        .filter(|s| s.starts_with('['))
        .collect();
    let entries = match r.entries {
        Some(entries) => entries(),
        None if params.iter().all(|p| p.starts_with("[[")) => {
            vec![vec![String::new(); params.len()]]
        }
        None => {
            return Err(format!(
                "{} has parameters: add `pub fn entries() -> Vec<String>` to its +page.rs to say which pages to export",
                r.pattern
            ));
        }
    };
    entries
        .iter()
        .map(|e| {
            segments(r.pattern, e).ok_or_else(|| {
                format!(
                    "{}: entries() gave {e:?}, which does not fit the route",
                    r.pattern
                )
            })
        })
        .collect()
}

/// `/blog/[slug]` and `["hi"]` → `["blog", "hi"]`; `None` if the values do
/// not fit, or would name a file outside the folder.
fn segments(pattern: &str, values: &[String]) -> Option<Vec<String>> {
    let mut values = values.iter();
    let mut out = Vec::new();
    for seg in pattern.split('/').filter(|s| !s.is_empty()) {
        if !seg.starts_with('[') {
            out.push(seg.to_string());
            continue;
        }
        let value = values.next()?;
        match (value.is_empty(), seg.starts_with("[...")) {
            (true, _) if seg.starts_with("[[") || seg.starts_with("[...") => {}
            (true, _) => return None,
            (false, true) => out.extend(value.split('/').map(String::from)),
            (false, false) => out.push(value.clone()),
        }
    }
    let safe = |s: &String| {
        !s.is_empty()
            && s != "."
            && s != ".."
            && !s.contains(|c: char| c.is_control() || "/\\:?#*\"<>|".contains(c))
    };
    (values.next().is_none() && out.iter().all(safe)).then_some(out)
}

/// The address to ask for: `/blog/hello%20world`.
pub(crate) fn url(segs: &[String]) -> String {
    if segs.is_empty() {
        return "/".into();
    }
    let mut s = String::new();
    for seg in segs {
        s.push('/');
        let _ = crate::cx::encode(&mut s, seg, crate::cx::unreserved);
    }
    s
}

/// The file a static host serves for the path: `/about` is `about/index.html`,
/// and `/feed.xml` is `feed.xml`.
fn file(segs: &[String]) -> String {
    match segs.last() {
        None => "index.html".into(),
        Some(last) if last.contains('.') => segs.join("/"),
        Some(_) => format!("{}/index.html", segs.join("/")),
    }
}

/// The `/_app/...` files a page refers to (scripts, CSS, browser modules).
fn find_assets(html: &str, out: &mut BTreeSet<String>) {
    let mut rest = html;
    while let Some(i) = rest.find(crate::protocol::APP_PREFIX) {
        let tail = &rest[i..];
        let end = tail
            .find(['"', '\'', '?', '<', '>', ' ', '\\', ')'])
            .unwrap_or(tail.len());
        out.insert(tail[..end].to_string());
        rest = &tail[end..];
    }
}

fn write(dir: &Path, rel: &str, bytes: &[u8]) -> io::Result<()> {
    let path = dir.join(rel);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&path, bytes)?;
    println!("wrote {rel}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn segs(pattern: &str, values: &[&str]) -> Option<Vec<String>> {
        segments(
            pattern,
            &values.iter().map(|v| v.to_string()).collect::<Vec<_>>(),
        )
    }

    fn route(pattern: &'static str, entries: Option<fn() -> Vec<Vec<String>>>) -> ExportRoute {
        ExportRoute {
            pattern,
            page: true,
            actions: false,
            server: false,
            indexed: true,
            entries,
            ssr: true,
            prerender: false,
        }
    }

    #[test]
    fn segments_fill_parameters() {
        assert_eq!(segs("/", &[]), Some(vec![]));
        assert_eq!(
            segs("/blog/[slug]", &["hi"]),
            Some(vec!["blog".into(), "hi".into()])
        );
        assert_eq!(
            segs("/[a]/x/[b]", &["1", "2"]),
            Some(vec!["1".into(), "x".into(), "2".into()])
        );
        assert_eq!(
            segs("/docs/[...path]", &["a/b"]),
            Some(vec!["docs".into(), "a".into(), "b".into()])
        );
        assert_eq!(segs("/docs/[...path]", &[""]), Some(vec!["docs".into()]));
        assert_eq!(segs("/p/[[n]]", &[""]), Some(vec!["p".into()]));
        assert_eq!(segs("/p/[[n]]", &["2"]), Some(vec!["p".into(), "2".into()]));
    }

    #[test]
    fn segments_refuse_what_does_not_fit() {
        assert_eq!(segs("/blog/[slug]", &[]), None);
        assert_eq!(segs("/blog/[slug]", &[""]), None);
        assert_eq!(segs("/blog/[slug]", &["a", "b"]), None);
        assert_eq!(segs("/blog", &["a"]), None);
        for bad in ["..", ".", "a/b", "a\\b", "c:", "a?b", "a#b", "a\0b"] {
            assert_eq!(segs("/blog/[slug]", &[bad]), None, "{bad:?}");
        }
        assert_eq!(segs("/x/[...p]", &["a/../b"]), None);
    }

    #[test]
    fn files_and_urls() {
        let s = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(file(&s(&[])), "index.html");
        assert_eq!(file(&s(&["about"])), "about/index.html");
        assert_eq!(file(&s(&["blog", "hi"])), "blog/hi/index.html");
        assert_eq!(file(&s(&["feed.xml"])), "feed.xml");
        assert_eq!(url(&s(&[])), "/");
        assert_eq!(
            url(&s(&["blog", "hello world", "é"])),
            "/blog/hello%20world/%C3%A9"
        );
    }

    #[test]
    fn routes_make_paths() {
        assert_eq!(paths(&route("/", None)), Ok(vec![vec![]]));
        assert_eq!(
            paths(&route("/a/b", None)),
            Ok(vec![vec!["a".into(), "b".into()]])
        );
        assert!(
            paths(&route("/blog/[slug]", None))
                .unwrap_err()
                .contains("entries()")
        );
        assert!(paths(&route("/[a]/[[b]]", None)).is_err());
        assert_eq!(paths(&route("/p/[[n]]", None)), Ok(vec![vec!["p".into()]]));
        assert_eq!(
            paths(&route(
                "/blog/[slug]",
                Some(|| vec!["a".params(), "b".params()])
            ))
            .unwrap()
            .len(),
            2
        );
        assert!(
            paths(&route("/blog/[slug]", Some(|| vec![("a", "b").params()])))
                .unwrap_err()
                .contains("does not fit")
        );
    }

    #[test]
    fn source_maps_beside_their_module() {
        assert_eq!(
            source_map("let a = 1\n//# sourceMappingURL=t3.js.map\n"),
            Some(("let a = 1\n", "t3.js.map"))
        );
        assert_eq!(source_map("let a = 1\n"), None);
        for odd in ["https://x/t.map", "../t.map", "data:x", ""] {
            assert_eq!(source_map(&format!("//# sourceMappingURL={odd}")), None);
        }
    }

    #[test]
    fn assets_are_found_in_pages() {
        let mut found = BTreeSet::new();
        find_assets(
            "<link href=\"/_app/app.css?v=1\"><script src=\"/_app/wisp.js?v=1\"></script>{\"m\":{\"t1\":\"/_app/c/t1.js?v=1\"}} /_app/live.js",
            &mut found,
        );
        assert_eq!(
            found.into_iter().collect::<Vec<_>>(),
            [
                "/_app/app.css",
                "/_app/c/t1.js",
                "/_app/live.js",
                "/_app/wisp.js"
            ]
        );
    }
}
