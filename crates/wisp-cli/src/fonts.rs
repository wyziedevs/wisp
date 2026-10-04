//! The fonts step of a build: a `google` line of `src/fonts.txt` (see
//! `wisp_shared::fonts`) is the app's opt-in to download that font, once,
//! from Google Fonts. Its Latin subset goes to `static/fonts/` as WOFF2, one
//! file per weight, which the app serves itself from then on; a TrueType
//! copy goes to `.wisp/fonts/` for the metrics the fallback face is sized by.
//! Nothing is fetched for a font already there, or without a `google` line.
//! A failure is a warning: the build goes on, and the font falls back.

use crate::{net, term};
use std::fs;
use std::path::Path;
use wisp_shared::fonts::{self, Face};

/// A browser Google serves WOFF2 to (the default `curl` gets TrueType).
const BROWSER: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36";

/// The stylesheet that names `family`'s `weight`.
fn api(family: &str, weight: &str, italic: bool) -> String {
    let family = family.replace(' ', "+");
    let axis = if italic { "ital,wght@1," } else { "wght@" };
    format!("https://fonts.googleapis.com/css2?family={family}:{axis}{weight}&display=swap")
}

/// The URL of the Latin subset in Google's stylesheet, else the first.
fn latin_url(css: &str) -> Option<String> {
    let from = css.find("/* latin */").unwrap_or(0);
    let rest = &css[from..];
    let at = rest.find("url(")? + 4;
    let end = rest[at..].find(')')?;
    Some(rest[at..at + end].trim_matches(['\'', '"']).to_string())
}

fn download(url: &str, dest: &Path, magic: &[u8]) -> Result<(), String> {
    if let Some(dir) = dest.parent() {
        fs::create_dir_all(dir).map_err(|e| format!("Could not make {}: {e}.", dir.display()))?;
    }
    let ok = |p: &Path| match fs::read(p) {
        Ok(b) if b.starts_with(magic) => Ok(()),
        _ => Err(format!("{url} is not the font it should be.")),
    };
    net::fetch_to(url, dest, ok)
}

fn one(root: &Path, f: &Face) -> Result<(), String> {
    let css = net::fetch_as(&api(&f.family, &f.weight, f.italic), BROWSER).map_err(|e| e.text)?;
    let css = String::from_utf8_lossy(&css);
    let url = latin_url(&css).ok_or("Google Fonts named no file")?;
    download(&url, &root.join("static/fonts").join(&f.file), b"wOF2")?;
    // The metrics come from a TrueType file; once for the family.
    let ttf = root
        .join(".wisp/fonts")
        .join(format!("{}.ttf", fonts::slug(&f.family)));
    if !ttf.exists() {
        let css = net::fetch(&api(&f.family, &f.weight, f.italic)).map_err(|e| e.text)?;
        if let Some(url) = latin_url(&String::from_utf8_lossy(&css)) {
            let _ = download(&url, &ttf, &[0, 1, 0, 0]);
        }
    }
    Ok(())
}

/// Downloads what the `google` lines name and is not there yet.
pub fn build(root: &Path) {
    let Ok(src) = fs::read_to_string(root.join("src/fonts.txt")) else {
        return;
    };
    let Ok(lines) = fonts::parse(&src) else {
        return; // the build says what is wrong
    };
    let google: Vec<_> = lines.into_iter().filter(|l| l.file.is_none()).collect();
    let mut got = 0;
    for f in fonts::faces(&google) {
        if root.join("static/fonts").join(&f.file).exists() {
            continue;
        }
        match one(root, &f) {
            Ok(()) => got += 1,
            Err(e) => term::warn(&format!(
                "Could not get {} {} from Google Fonts: {e} The page uses its fallback font.",
                f.family, f.weight
            )),
        }
    }
    if got > 0 {
        term::done(&format!("Downloaded {got} font file(s) to static/fonts"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls_and_the_latin_subset() {
        assert_eq!(
            api("Open Sans", "700", true),
            "https://fonts.googleapis.com/css2?family=Open+Sans:ital,wght@1,700&display=swap"
        );
        let css = "/* cyrillic */\n@font-face { src: url(https://x/a.woff2) format('woff2'); }\n/* latin */\n@font-face { src: url(https://x/b.woff2) format('woff2'); }\n";
        assert_eq!(latin_url(css).as_deref(), Some("https://x/b.woff2"));
        assert_eq!(
            latin_url("src: url('https://x/c.ttf');").as_deref(),
            Some("https://x/c.ttf")
        );
        assert_eq!(latin_url("nothing"), None);
    }
}
