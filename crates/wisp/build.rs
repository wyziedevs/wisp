//! Release builds serve the browser runtime (`src/client/wisp.js`,
//! `live.js`) without its comments and indentation: a third less over the
//! wire. Dev builds serve the files as written, for debugging.
//!
//! Only whole lines are touched, so strings and regexes are never looked
//! into: a line that is a `//` comment goes, as does a `/* ... */` block
//! that starts a line, and every line loses its leading whitespace. Inside
//! a template literal that spans lines, nothing is changed.

use std::path::Path;

fn main() {
    let out = std::env::var("OUT_DIR").expect("cargo sets OUT_DIR");
    for name in ["wisp.js", "live.js"] {
        let src = Path::new("src/client").join(name);
        println!("cargo:rerun-if-changed={}", src.display());
        let js = std::fs::read_to_string(&src).expect("the client runtime");
        std::fs::write(Path::new(&out).join(name), strip(&js)).expect("OUT_DIR is writable");
    }
}

fn strip(js: &str) -> String {
    let mut out = String::with_capacity(js.len());
    let mut in_template = false;
    let mut in_block = false;
    for line in js.lines() {
        if in_template {
            out.push_str(line);
            out.push('\n');
            in_template = odd_backticks(line) != in_template;
            continue;
        }
        let mut t = line.trim_start();
        if in_block || t.starts_with("/*") {
            // What follows the comment's end on its line is code.
            match t.find("*/") {
                Some(end) => (t, in_block) = (t[end + 2..].trim_start(), false),
                None => {
                    in_block = true;
                    continue;
                }
            }
        }
        if t.is_empty() || t.starts_with("//") {
            continue;
        }
        out.push_str(t);
        out.push('\n');
        in_template = odd_backticks(t);
    }
    out
}

/// Whether `line` opens or closes a template literal: an odd count of
/// unescaped backticks.
fn odd_backticks(line: &str) -> bool {
    let b = line.as_bytes();
    (0..b.len())
        .filter(|&i| b[i] == b'`' && (i == 0 || b[i - 1] != b'\\'))
        .count()
        % 2
        == 1
}
