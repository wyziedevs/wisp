//! `/sitemap.xml` and `/robots.txt`, made from the routes when no route
//! and no file in `static/` answers them.
//!
//! The sitemap lists every page whose address is known: a route without
//! parameters, or one with `entries()`, outside any `(private)` group and
//! without `<meta name="robots" content="noindex">`. Addresses start with
//! `SITE_URL`, else the request's host.

use crate::export::{ExportRoute, paths, url};
use crate::{App, Cx};
use std::collections::BTreeSet;
use std::panic::{AssertUnwindSafe, catch_unwind};

/// The body and type of `/sitemap.xml` or `/robots.txt`; `None` for
/// another path, or an app with no page to list.
pub(crate) fn answer<A: App>(cx: &Cx) -> Option<(Vec<u8>, &'static str)> {
    let sitemap = match cx.raw_path() {
        b"/sitemap.xml" => true,
        b"/robots.txt" => false,
        _ => return None,
    };
    let routes = A::export_routes();
    let base = base(cx).filter(|_| routes.iter().any(|r| r.page && r.indexed))?;
    let (body, mime) = match sitemap {
        true => (xml(&base, &routes), "application/xml; charset=utf-8"),
        false => (robots(&base), "text/plain; charset=utf-8"),
    };
    Some((body.into_bytes(), mime))
}

/// Every crawler may read every page, and where the sitemap is.
fn robots(base: &str) -> String {
    format!("User-agent: *\nAllow: /\n\nSitemap: {base}/sitemap.xml\n")
}

/// `https://example.com`: `SITE_URL` without its last `/`, else the
/// request's scheme and host; `None` with neither.
fn base(cx: &Cx) -> Option<String> {
    if let Ok(site) = std::env::var("SITE_URL")
        && !site.is_empty()
    {
        return Some(site.trim_end_matches('/').to_string());
    }
    let host = cx.header("host").filter(|h| {
        !h.is_empty()
            && h.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b".-:[]".contains(&b))
    })?;
    let local = ["localhost", "127.0.0.1", "[::1]"]
        .iter()
        .any(|l| host == *l || host.starts_with(&format!("{l}:")));
    let scheme = match cx.header("x-forwarded-proto") {
        Some(p @ ("http" | "https")) => p,
        _ if local => "http",
        _ => "https",
    };
    Some(format!("{scheme}://{host}"))
}

/// The sitemap of `routes`' indexed pages, each address once, in order.
fn xml(base: &str, routes: &[ExportRoute]) -> String {
    let mut urls = BTreeSet::new();
    for r in routes.iter().filter(|r| r.page && r.indexed) {
        // A route whose `entries()` panics or does not fit is left out.
        if let Ok(Ok(all)) = catch_unwind(AssertUnwindSafe(|| paths(r))) {
            urls.extend(all.iter().map(|segs| url(segs)));
        }
    }
    let mut out = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<urlset xmlns=\"http://www.sitemaps.org/schemas/sitemap/0.9\">\n",
    );
    for u in urls {
        out.push_str("<url><loc>");
        crate::html::text(&mut out, &format!("{base}{u}"));
        out.push_str("</loc></url>\n");
    }
    out.push_str("</urlset>\n");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn route(pattern: &'static str, indexed: bool) -> ExportRoute {
        ExportRoute {
            pattern,
            page: true,
            actions: false,
            server: false,
            entries: None,
            indexed,
            ssr: true,
        }
    }

    #[test]
    fn lists_known_addresses() {
        fn slugs() -> Vec<Vec<String>> {
            vec![vec!["a&b".into()], vec!["c".into()]]
        }
        let mut post = route("/post/[slug]", true);
        post.entries = Some(slugs);
        let routes = [
            route("/", true),
            route("/about", true),
            route("/secret", false),
            route("/user/[id]", true),
            route("/[[lang]]/docs", true),
            post,
        ];
        assert_eq!(
            xml("https://x.org", &routes),
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
             <urlset xmlns=\"http://www.sitemaps.org/schemas/sitemap/0.9\">\n\
             <url><loc>https://x.org/</loc></url>\n\
             <url><loc>https://x.org/about</loc></url>\n\
             <url><loc>https://x.org/docs</loc></url>\n\
             <url><loc>https://x.org/post/a%26b</loc></url>\n\
             <url><loc>https://x.org/post/c</loc></url>\n\
             </urlset>\n"
        );
        assert_eq!(
            robots("https://x.org"),
            "User-agent: *\nAllow: /\n\nSitemap: https://x.org/sitemap.xml\n"
        );
    }
}
