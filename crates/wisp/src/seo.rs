//! `/sitemap.xml`, `/robots.txt` and `/feed.xml`, made from the routes when
//! no route and no file in `static/` answers them. The feed is an Atom
//! feed of the Markdown pages with a `date` (newest first; `SITE_TITLE`
//! names it, a page's `description` field is its summary). [`og`] is the
//! Open Graph tags of a page's head.
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
        b"/feed.xml" => {
            let slashed = crate::http::slash() == crate::TrailingSlash::Always;
            let feed = atom(&base(cx)?, crate::content::all(), slashed)?;
            return Some((feed.into_bytes(), "application/atom+xml; charset=utf-8"));
        }
        b"/sitemap.xml" => true,
        b"/robots.txt" => false,
        _ => return None,
    };
    let routes = A::export_routes();
    let base = base(cx).filter(|_| routes.iter().any(|r| r.page && r.indexed))?;
    let (body, mime) = match sitemap {
        true => (
            xml(
                &base,
                &routes,
                crate::http::slash() == crate::TrailingSlash::Always,
            ),
            "application/xml; charset=utf-8",
        ),
        false => (robots(&base), "text/plain; charset=utf-8"),
    };
    Some((body.into_bytes(), mime))
}

/// `v` as XML text: escaped, and without the control characters XML 1.0
/// cannot hold at all (a feed reader rejects the whole file for one).
fn esc(out: &mut String, v: &str) {
    let bad = |c: char| c.is_control() && !matches!(c, '\t' | '\n' | '\r');
    if v.chars().any(bad) {
        let v: String = v.chars().filter(|&c| !bad(c)).collect();
        return crate::html::text(out, &v);
    }
    crate::html::text(out, v);
}

/// `<tag>v</tag>`, escaped.
fn elem(out: &mut String, tag: &str, v: &str) {
    out.push_str(&format!("<{tag}>"));
    esc(out, v);
    out.push_str(&format!("</{tag}>"));
}

/// The most addresses a sitemap may hold (the protocol's own limit).
const MAX_URLS: usize = 50_000;

/// The Atom feed of the dated pages in `all`, newest first; `None` with
/// none. `slashed`: addresses end in `/`, as in the sitemap.
fn atom(base: &str, all: &[crate::MdPage], slashed: bool) -> Option<String> {
    let mut dated: Vec<_> = all
        .iter()
        .filter_map(|p| Some((p.get("date")?, p)))
        .collect();
    dated.sort_by(|a, b| b.0.cmp(a.0));
    let when = |d: &str| match d.len() {
        10 => format!("{d}T00:00:00Z"),
        _ => d.to_string(),
    };
    let site = std::env::var("SITE_TITLE")
        .unwrap_or_else(|_| base.split_once("://").map_or(base, |h| h.1).to_string());
    let mut out = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<feed xmlns=\"http://www.w3.org/2005/Atom\">\n",
    );
    elem(&mut out, "title", &site);
    elem(&mut out, "id", &format!("{base}/"));
    elem(&mut out, "updated", &when(dated.first()?.0));
    out.push('\n');
    for (date, p) in dated {
        let segs: Vec<String> = (p.path.split('/'))
            .filter(|s| !s.is_empty())
            .map(String::from)
            .collect();
        let end = if slashed && !segs.is_empty() { "/" } else { "" };
        let url = format!("{base}{}{end}", url(&segs));
        out.push_str("<entry>");
        elem(&mut out, "title", p.title);
        out.push_str("<link href=\"");
        esc(&mut out, &url);
        out.push_str("\"/>");
        elem(&mut out, "id", &url);
        elem(&mut out, "updated", &when(date));
        if let Some(d) = p.get("description") {
            elem(&mut out, "summary", d);
        }
        out.push_str("</entry>\n");
    }
    out.push_str("</feed>\n");
    Some(out)
}

/// A page's Open Graph and Twitter card tags, for its head:
/// `{@html wisp::og("Hello", "A first post", "/cover.png")}`. Escaped; an
/// empty `image` leaves that tag out. `"auto"` is an SVG, or with the
/// `og-png` feature (also on `wisp-cli`) a PNG, which crawlers take more often.
pub fn og(title: &str, description: &str, image: &str) -> String {
    // `"auto"`: the picture `wisp build` made of this title (SVG, see `wisp_shared::og`).
    let auto = wisp_shared::og::url(title);
    let image = if image == "auto" { &auto } else { image };
    let mut out = String::new();
    let mut tag = |name: &str, v: &str| {
        out.push_str(&format!("<meta property=\"{name}\" content=\""));
        crate::html::text(&mut out, v);
        out.push_str("\">\n");
    };
    tag("og:title", title);
    tag("og:description", description);
    if !image.is_empty() {
        tag("og:image", image);
    }
    let card = if image.is_empty() {
        "summary"
    } else {
        "summary_large_image"
    };
    out.push_str(&format!(
        "<meta name=\"twitter:card\" content=\"{card}\">\n"
    ));
    out
}

/// Every crawler may read every page, and where the sitemap is.
fn robots(base: &str) -> String {
    format!("User-agent: *\nAllow: /\n\nSitemap: {base}/sitemap.xml\n")
}

/// `https://example.com`: `SITE_URL` without its last `/`, else the
/// request's scheme and host (and base path); `None` with neither.
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
    Some(format!("{scheme}://{host}{}", crate::protocol::BASE))
}

/// The sitemap of `routes`' indexed pages, each address once, in order;
/// `slashed`: each ends in `/`, as `trailing_slash(Always)` serves them.
fn xml(base: &str, routes: &[ExportRoute], slashed: bool) -> String {
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
    for u in urls.into_iter().take(MAX_URLS) {
        out.push_str("<url><loc>");
        let end = if slashed && u != "/" { "/" } else { "" };
        esc(&mut out, &format!("{base}{u}{end}"));
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
            prerender: false,
        }
    }

    #[test]
    fn a_feed_and_og_tags() {
        let page = |path, date: Option<&'static str>| crate::MdPage {
            path,
            title: "A & B",
            fields: match date {
                Some(_) => &[("date", "2026-10-01"), ("description", "Hi")],
                None => &[],
            },
        };
        let all = [page("/blog/a", Some("")), page("/about", None)];
        let feed = atom("https://x.org", &all, false).unwrap();
        assert!(feed.contains("<updated>2026-10-01T00:00:00Z</updated>"));
        assert!(feed.contains("<link href=\"https://x.org/blog/a\"/>"));
        assert!(feed.contains("<title>A &amp; B</title>"));
        assert!(feed.contains("<summary>Hi</summary>"));
        assert!(!feed.contains("/about"));
        assert!(atom("https://x.org", &all[1..], false).is_none());
        // The address is encoded, and slashed as the sitemap's is.
        let odd = [page("/blog/a b", Some(""))];
        let feed = atom("https://x.org", &odd, true).unwrap();
        assert!(feed.contains("<link href=\"https://x.org/blog/a%20b/\"/>"));
        // XML cannot hold control characters.
        let mut out = String::new();
        elem(&mut out, "t", "a\u{c}b\u{0}<");
        assert_eq!(out, "<t>ab&lt;</t>");
        let tags = og("A \"b\"", "d", "");
        assert!(tags.contains("og:title\" content=\"A &quot;b&quot;\""));
        assert!(!tags.contains("og:image") && tags.contains("\"summary\""));
        assert!(og("t", "d", "/c.png").contains("og:image\" content=\"/c.png\""));
        assert!(og("Hi there", "d", "auto").contains("og:image\" content=\"/og/hi-there.svg\""));
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
            xml("https://x.org", &routes, false),
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
             <urlset xmlns=\"http://www.sitemaps.org/schemas/sitemap/0.9\">\n\
             <url><loc>https://x.org/</loc></url>\n\
             <url><loc>https://x.org/about</loc></url>\n\
             <url><loc>https://x.org/docs</loc></url>\n\
             <url><loc>https://x.org/post/a%26b</loc></url>\n\
             <url><loc>https://x.org/post/c</loc></url>\n\
             </urlset>\n"
        );
        let slashed = xml("https://x.org", &routes, true);
        assert!(slashed.contains("<loc>https://x.org/about/</loc>"));
        assert!(slashed.contains("<loc>https://x.org/post/c/</loc>"));
        assert!(slashed.contains("<loc>https://x.org/</loc>"));
        assert_eq!(
            robots("https://x.org"),
            "User-agent: *\nAllow: /\n\nSitemap: https://x.org/sitemap.xml\n"
        );
    }
}
