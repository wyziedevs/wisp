//! Wisp's compiler. An app's `build.rs` calls [`run`], which scans
//! `src/routes`, compiles every `.wisp` template and writes
//! `$OUT_DIR/wisp.rs` for `wisp::app!()` to include.

mod codegen;
pub mod routes;
pub mod rust_scan;
mod shell;
pub mod template;

use std::env;
use std::fs;
use std::path::{Path, PathBuf};

/// Entry point for an app's `build.rs`.
pub fn run() {
    let root = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("wisp_build::run must be called from build.rs"));
    let out_dir = PathBuf::from(env::var_os("OUT_DIR").expect("wisp_build::run must be called from build.rs"));
    let release = env::var("PROFILE").is_ok_and(|p| p == "release");

    // Only existing paths: Cargo treats a missing one as always changed, which
    // would rebuild the app on every `cargo build`.
    for p in ["src", "static", ".wisp/app.css"] {
        if root.join(p).exists() {
            println!("cargo::rerun-if-changed={p}");
        }
    }

    match codegen::generate(&codegen::Input { root: &root, release }) {
        Ok(code) => write_if_changed(&out_dir.join("wisp.rs"), &code),
        Err(e) => {
            eprintln!("\nwisp: {e}\n");
            std::process::exit(1);
        }
    }
}

/// For `wisp dev`: the static chunks and shape of the template at `rel`
/// (relative to the project root, `/`-separated). A running dev build can
/// take new chunks without recompiling as long as the shape is unchanged.
pub fn hot_chunks(root: &Path, rel: &str) -> Result<(Vec<String>, u64), String> {
    let src = fs::read_to_string(root.join(rel)).map_err(|e| format!("{rel}: {e}"))?;
    if rel == "src/app.html" {
        let parts = shell::split(&src).map_err(|e| format!("{rel}: {e}"))?;
        return Ok((parts.to_vec(), shell::SHAPE));
    }
    let t = template::parse(&src).map_err(|e| format!("{rel}:{e}"))?;
    Ok((t.chunks, t.shape))
}

/// Checks the whole project the way `run` does, without writing anything.
pub fn check(root: &Path) -> Result<(), String> {
    codegen::generate(&codegen::Input { root, release: false }).map(|_| ())
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
