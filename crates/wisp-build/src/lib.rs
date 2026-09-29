//! Wisp's compiler. An app's `build.rs` calls [`run`], which scans
//! `src/routes`, compiles every `.wisp` template and writes
//! `$OUT_DIR/wisp.rs` for `wisp::app!()` to include.

mod codegen;
mod js;
mod openapi;
pub mod routes;
pub mod rust_scan;
mod shell;
pub mod template;

/// JavaScript without comments and needless whitespace: the browser
/// runtime as release builds serve it (`wisp`'s build.rs).
pub use js::minify as minify_js;

use std::env;
use std::fs;
use std::path::{Path, PathBuf};

/// Entry point for an app's `build.rs`.
pub fn run() {
    let root = PathBuf::from(
        env::var_os("CARGO_MANIFEST_DIR").expect("wisp_build::run must be called from build.rs"),
    );
    let out_dir = PathBuf::from(
        env::var_os("OUT_DIR").expect("wisp_build::run must be called from build.rs"),
    );
    let release = env::var("PROFILE").is_ok_and(|p| p == "release");

    // Only existing paths: Cargo treats a missing one as always changed, which
    // would rebuild the app on every `cargo build`.
    for p in ["src", "static", ".wisp/app.css"] {
        if root.join(p).exists() {
            println!("cargo::rerun-if-changed={p}");
        }
    }

    match codegen::generate(&codegen::Input {
        root: &root,
        release,
    }) {
        Ok(code) => write_if_changed(&out_dir.join("wisp.rs"), &code),
        Err(e) => {
            eprintln!("\nwisp: {e}\n");
            std::process::exit(1);
        }
    }
}

/// A source file's text with `\r\n` (Windows checkouts) as `\n` and no byte
/// order mark, so that a template compiles to the same code either way.
pub fn read_source(path: &Path) -> std::io::Result<String> {
    let text = fs::read_to_string(path)?;
    let text = text.strip_prefix('\u{feff}').unwrap_or(&text);
    Ok(if text.contains('\r') {
        text.replace("\r\n", "\n")
    } else {
        text.to_string()
    })
}

/// For `wisp dev`: the static chunks and shape of the template at `rel`
/// (relative to the project root, `/`-separated). A running dev build can
/// take new chunks without recompiling as long as the shape is unchanged.
pub fn hot_chunks(root: &Path, rel: &str) -> Result<(Vec<String>, u64), String> {
    let src = read_source(&root.join(rel)).map_err(|e| format!("{rel}: {e}"))?;
    if rel == "src/app.html" {
        let parts = shell::split(&src).map_err(|e| format!("{rel}: {e}"))?;
        return Ok((parts.to_vec(), shell::SHAPE));
    }
    let (t, _) = parse_wisp(&src).map_err(|e| format!("{rel}:{e}"))?;
    Ok((t.chunks, t.shape))
}

/// A `.wisp` file parsed: its template, and the Rust of its `---` block if
/// it has one, as long as the file (the markup blanked, so every line of
/// Rust is on its line in the file). The block's text is part of the shape:
/// changing it means compiling again. Errors are `line:col: msg`.
pub fn parse_wisp(src: &str) -> Result<(template::Template, Option<String>), String> {
    let (rust, markup) = split_front(src)?;
    let mut t = template::parse(&markup).map_err(|e| e.to_string())?;
    if let Some(r) = &rust {
        t.shape ^= fnv1a(r.as_bytes()).rotate_left(1);
    }
    Ok((t, rust))
}

/// Splits off the `---` block of Rust a page or layout may start with:
///
/// ```text
/// ---
/// let post = posts::find(&slug).or_404()?;
/// ---
/// <h1>{post.title}</h1>
/// ```
///
/// Both halves keep the file's lines: the Rust with the markup blanked, the
/// markup with the block's lines left empty.
fn split_front(src: &str) -> Result<(Option<String>, String), String> {
    let lines: Vec<&str> = src.split('\n').collect();
    let Some(open) = lines.iter().position(|l| !l.trim().is_empty()) else {
        return Ok((None, src.to_string()));
    };
    if lines[open].trim() != "---" {
        return Ok((None, src.to_string()));
    }
    let Some(close) = lines[open + 1..].iter().position(|l| l.trim() == "---") else {
        return Err(format!(
            "{}:1: this `---` starts a block of Rust, which needs a `---` line after it",
            open + 1
        ));
    };
    let close = open + 1 + close;
    let pick = |rust: bool| -> String {
        let kept: Vec<&str> = lines
            .iter()
            .enumerate()
            .map(|(k, l)| {
                let inside = k > open && k < close;
                let markup = k > close;
                if (rust && inside) || (!rust && markup) {
                    *l
                } else {
                    ""
                }
            })
            .collect();
        kept.join("\n")
    };
    Ok((Some(pick(true)), pick(false)))
}

/// Checks the whole project the way `run` does, without writing anything.
pub fn check(root: &Path) -> Result<(), String> {
    codegen::generate(&codegen::Input {
        root,
        release: false,
    })
    .map(|_| ())
}

/// FNV-1a, 64-bit. Used for shape and asset hashes, not for security.
pub const fn fnv1a(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let mut i = 0;
    while i < bytes.len() {
        h ^= bytes[i] as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
        i += 1;
    }
    h
}

/// Rewriting identical output would bump its mtime and trigger rebuilds.
fn write_if_changed(path: &Path, contents: &str) {
    if fs::read(path).is_ok_and(|old| old == contents.as_bytes()) {
        return;
    }
    fs::write(path, contents).unwrap_or_else(|e| panic!("writing {}: {e}", path.display()));
}

#[cfg(test)]
mod tests {
    #[test]
    fn fnv_vectors() {
        assert_eq!(super::fnv1a(b""), 0xcbf29ce484222325);
        assert_eq!(super::fnv1a(b"a"), 0xaf63dc4c8601ec8c);
        assert_eq!(super::fnv1a(b"foobar"), 0x85944171f73967e8);
    }
}
