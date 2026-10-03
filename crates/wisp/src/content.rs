//! Markdown pages' front matter, for index pages: [`pages`].

use std::sync::OnceLock;

/// A Markdown page (`+page.md`, `blog/x.md`) as [`pages`] lists it.
#[derive(Debug)]
pub struct MdPage {
    /// Its address: `/blog/hello`.
    pub path: &'static str,
    /// Its `title` (from the front matter, or its first `# heading`), or
    /// empty.
    pub title: &'static str,
    /// Its front matter, in order: `("date", "2026-10-01")`.
    pub fields: &'static [(&'static str, &'static str)],
}

impl MdPage {
    /// A front matter field: `p.get("date")`; `None` if it has none.
    pub fn get(&self, name: &str) -> Option<&'static str> {
        self.fields
            .iter()
            .find(|(k, _)| *k == name)
            .map(|(_, v)| *v)
    }
}

static PAGES: OnceLock<&'static [MdPage]> = OnceLock::new();

/// Keeps the app's pages (by folder, then newest `date` first, then by
/// path, as the build sorted them) for [`pages`].
pub(crate) fn ready(all: &'static [MdPage]) {
    let _ = PAGES.set(all);
}

/// The Markdown pages in folder `dir` (`"blog"` for `/blog/x`; `""` for the
/// root), newest `date` first, then by path:
/// `{#each wisp::pages("blog") as p}<a href={p.path}>{p.title}</a>{/each}`.
pub fn pages(dir: &str) -> &'static [MdPage] {
    let all = PAGES.get().copied().unwrap_or_default();
    let dir = dir.trim_matches('/');
    let of = |p: &MdPage| {
        let d = p.path.rsplit_once('/').map_or("", |(d, _)| d);
        d.trim_start_matches('/')
    };
    let start = all.iter().position(|p| of(p) == dir).unwrap_or(all.len());
    let len = all[start..].iter().take_while(|p| of(p) == dir).count();
    &all[start..start + len]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pages_of_a_folder() {
        static ALL: [MdPage; 3] = [
            MdPage {
                path: "/about",
                title: "About",
                fields: &[],
            },
            MdPage {
                path: "/blog/b",
                title: "B",
                fields: &[("date", "2026-02")],
            },
            MdPage {
                path: "/blog/a",
                title: "A",
                fields: &[("date", "2026-01")],
            },
        ];
        ready(&ALL);
        let blog: Vec<_> = pages("/blog/").iter().map(|p| p.title).collect();
        assert_eq!(blog, ["B", "A"]);
        assert_eq!(pages("")[0].path, "/about");
        assert!(pages("none").is_empty());
        assert_eq!(pages("blog")[0].get("date"), Some("2026-02"));
        assert_eq!(pages("blog")[0].get("x"), None);
    }
}
