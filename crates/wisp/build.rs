//! Release builds serve the browser runtime (`wisp_shared::WISP_JS`,
//! `LIVE_JS`) minified: no comments, only the whitespace JavaScript needs,
//! and short names (see `wisp_build::minify_js`). Dev builds serve the
//! files as written, for debugging, so they minify nothing.

use std::path::Path;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    // The `?v=` the runtime is linked by: wisp-build writes the same one
    // into the modules it generates.
    println!(
        "cargo:rustc-env=WISP_RUNTIME_V={}",
        wisp_build::runtime_version()
    );
    // What `http.rs` serves by: `cfg(debug_assertions)`, the files as written.
    if std::env::var_os("CARGO_CFG_DEBUG_ASSERTIONS").is_some() {
        return;
    }
    let out = std::env::var("OUT_DIR").expect("cargo sets OUT_DIR");
    for (name, js) in [
        ("wisp.js", wisp_shared::WISP_JS),
        ("live.js", wisp_shared::LIVE_JS),
    ] {
        std::fs::write(Path::new(&out).join(name), wisp_build::minify_js(js))
            .expect("OUT_DIR is writable");
    }
}
