//! The Open Graph step of a release build: each `wisp::og("Title", "About",
//! "auto")` in `src` with a literal title and description gets its picture
//! at `static/og/<slug>.svg` (see `wisp_shared::og`), drawn with the
//! colors of `src/app.css`'s `--bg`, `--ink` and `--accent` and the app's
//! name. A file is written only when it would change.

use crate::{cargo, term};
use std::fs;
use std::path::{Path, PathBuf};
use wisp_shared::og::{self, Look};

fn sources(dir: &Path, out: &mut Vec<PathBuf>, depth: usize) {
    for e in fs::read_dir(dir).into_iter().flatten().flatten() {
        let p = e.path();
        if p.is_dir() && depth < 32 {
            sources(&p, out, depth + 1);
        } else if ["wisp", "md", "rs"]
            .iter()
            .any(|x| p.extension().is_some_and(|e| e == *x))
        {
            out.push(p);
        }
    }
}

/// Writes the pictures; never fails the build.
pub fn build(root: &Path) {
    let mut files = Vec::new();
    sources(&root.join("src"), &mut files, 0);
    let mut found: Vec<(String, String)> = Vec::new();
    for f in &files {
        let Ok(src) = fs::read_to_string(f) else {
            continue;
        };
        for call in og::calls(&src) {
            if !found.iter().any(|(t, _)| og::slug(t) == og::slug(&call.0)) {
                found.push(call);
            }
        }
    }
    if found.is_empty() {
        return;
    }
    let css = fs::read_to_string(root.join("src/app.css")).unwrap_or_default();
    let token = |names: &[&str]| names.iter().find_map(|n| og::token(&css, n));
    let (bg, ink, accent) = (
        token(&["bg"]),
        token(&["ink", "fg", "text"]),
        token(&["accent", "brand"]),
    );
    let brand = cargo::package_name(root).unwrap_or_default();
    let d = Look::DEFAULT;
    let look = Look {
        bg: bg.as_deref().unwrap_or(d.bg),
        ink: ink.as_deref().unwrap_or(d.ink),
        accent: accent.as_deref().unwrap_or(d.accent),
        brand: &brand,
    };
    let dir = root.join("static/og");
    let mut wrote = 0;
    for (title, desc) in &found {
        let path = dir.join(format!("{}.svg", og::slug(title)));
        let svg = og::svg(title, desc, &look);
        let same = fs::read_to_string(&path).is_ok_and(|old| old == svg);
        #[cfg(feature = "og-png")]
        if !same || !path.with_extension("png").is_file() {
            match png(&svg) {
                Some(b) => {
                    let _ = fs::create_dir_all(&dir);
                    if fs::write(path.with_extension("png"), b).is_err() {
                        term::warn(&format!("Could not write {}.", path.display()));
                    }
                }
                None => term::warn(&format!("Could not render the PNG of {title:?}.")),
            }
        }
        if same {
            continue;
        }
        if fs::create_dir_all(&dir).is_err() || fs::write(&path, svg).is_err() {
            term::warn(&format!("Could not write {}.", path.display()));
            return;
        }
        wrote += 1;
    }
    if wrote > 0 {
        term::done(&format!("Wrote {wrote} Open Graph image(s) to static/og"));
    }
}

/// The PNG of the picture (the `og-png` feature), text drawn with the
/// system's fonts: `None` when it cannot be drawn.
#[cfg(feature = "og-png")]
fn png(svg: &str) -> Option<Vec<u8>> {
    use resvg::{tiny_skia, usvg};
    let mut opt = usvg::Options::default();
    opt.fontdb_mut().load_system_fonts();
    let tree = usvg::Tree::from_str(svg, &opt).ok()?;
    let size = tree.size().to_int_size();
    let mut pixmap = tiny_skia::Pixmap::new(size.width(), size.height())?;
    resvg::render(
        &tree,
        tiny_skia::Transform::identity(),
        &mut pixmap.as_mut(),
    );
    pixmap.encode_png().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_the_picture_of_each_literal_auto_call() {
        let root = std::env::temp_dir().join(format!("wisp-og-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("src/routes")).unwrap();
        let page = "{@html wisp::og(\"Hello there\", \"A first post\", \"auto\")}";
        fs::write(root.join("src/routes/+page.wisp"), page).unwrap();
        fs::write(root.join("src/app.css"), ":root { --accent: #ff0000; }").unwrap();
        build(&root);
        let svg = fs::read_to_string(root.join("static/og/hello-there.svg")).unwrap();
        assert!(svg.contains("Hello there") && svg.contains("#ff0000"));
        #[cfg(feature = "og-png")]
        assert_eq!(
            &fs::read(root.join("static/og/hello-there.png")).unwrap()[1..4],
            b"PNG"
        );
        let _ = fs::remove_dir_all(&root);
    }
}
