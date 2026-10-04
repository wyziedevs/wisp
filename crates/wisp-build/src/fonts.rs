//! `src/fonts.txt` in the build (see `wisp_shared::fonts`): the CSS that goes
//! first in `/_app/app.css` and the preload links of the shell's head.

use std::fs;
use std::path::Path;
use wisp_shared::fonts::{self, Face, Metrics};

/// The app's fonts, none without a `src/fonts.txt`.
pub fn load(root: &Path) -> Result<Vec<Face>, String> {
    let Ok(src) = fs::read_to_string(root.join("src").join("fonts.txt")) else {
        return Ok(Vec::new());
    };
    Ok(fonts::faces(&fonts::parse(&src)?))
}

/// Where a face's metrics are read from: the file itself when it is a TrueType
/// or OpenType one, else the copy `wisp build` downloaded for them.
fn metrics(root: &Path, f: &Face) -> Option<Metrics> {
    let own = fs::read(root.join("static/fonts").join(&f.file)).ok();
    let cached = || {
        fs::read(
            root.join(".wisp/fonts")
                .join(format!("{}.ttf", fonts::slug(&f.family))),
        )
        .ok()
    };
    own.as_deref()
        .and_then(fonts::metrics)
        .or_else(|| fonts::metrics(&cached()?))
}

/// The `@font-face` rules and `--font-*` properties, or nothing.
pub fn css(root: &Path) -> String {
    let faces = load(root).unwrap_or_default();
    fonts::css(&faces, crate::protocol::BASE, |f| metrics(root, f))
}
