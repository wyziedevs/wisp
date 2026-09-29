//! Release builds serve the browser runtime (`src/client/wisp.js`,
//! `live.js`) minified: no comments, only the whitespace JavaScript needs,
//! and short names (see `wisp_build::minify_js`). Dev builds serve the
//! files as written, for debugging.

use std::path::Path;

fn main() {
    let out = std::env::var("OUT_DIR").expect("cargo sets OUT_DIR");
    for name in ["wisp.js", "live.js"] {
        let src = Path::new("src/client").join(name);
        println!("cargo:rerun-if-changed={}", src.display());
        let js = std::fs::read_to_string(&src).expect("the client runtime");
        std::fs::write(Path::new(&out).join(name), wisp_build::minify_js(&js))
            .expect("OUT_DIR is writable");
    }
}
