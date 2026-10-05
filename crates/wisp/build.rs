//! Release builds serve the browser runtime (`wisp_shared::WISP_JS`,
//! `LIVE_JS`) minified: no comments, only the whitespace JavaScript needs,
//! and short names (`wisp_shared::WISP_MIN_JS`, made by `wisp_build::minify_js`).
//! Dev builds serve the files as written, for debugging. No `wisp-build`
//! here: it would make this crate wait for it to compile.

use std::path::Path;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    // The `?v=` the runtime is linked by: wisp-build writes the same one
    // into the modules it generates.
    println!(
        "cargo:rustc-env=WISP_RUNTIME_V={}",
        wisp_shared::runtime_version()
    );
    // The base path the app is served under (see `protocol::BASE`).
    println!("cargo:rustc-env=WISP_BASE={}", wisp_shared::protocol::BASE);
    // `WISP_REQUEST_ONLY=1` (`wisp build` sets it for Cloudflare, Pages, Vercel
    // and Netlify, whose hosts hand over whole requests): the wasm32 build
    // leaves out the server loop for raw connections, 25 KB it never runs.
    println!("cargo:rustc-check-cfg=cfg(request_only)");
    println!("cargo:rerun-if-env-changed=WISP_REQUEST_ONLY");
    if std::env::var_os("WISP_REQUEST_ONLY").is_some_and(|v| v == "1") {
        println!("cargo:rustc-cfg=request_only");
    }
    // `WISP_JOBS=0` (`wisp build` sets it when the app's `src/` has no
    // `wisp::cron` and no `wisp::work`, so the host is given no trigger): the
    // wasm32 build leaves out the trigger's route and the jobs it runs.
    println!("cargo:rustc-check-cfg=cfg(no_jobs)");
    println!("cargo:rerun-if-env-changed=WISP_JOBS");
    if std::env::var_os("WISP_JOBS").is_some_and(|v| v == "0") {
        println!("cargo:rustc-cfg=no_jobs");
    }
    // What `http.rs` serves by: `cfg(debug_assertions)`, the files as written.
    if std::env::var_os("CARGO_CFG_DEBUG_ASSERTIONS").is_some() {
        return;
    }
    let out = std::env::var("OUT_DIR").expect("cargo sets OUT_DIR");
    for (name, js) in [
        ("wisp.js", wisp_shared::WISP_MIN_JS),
        ("live.js", wisp_shared::LIVE_MIN_JS),
    ] {
        std::fs::write(Path::new(&out).join(name), js).expect("OUT_DIR is writable");
    }
}
