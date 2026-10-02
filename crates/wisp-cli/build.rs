//! Writes the `wisp new` templates' file tables into OUT_DIR: one folder is
//! the one source of each, so a file added there reaches new apps. With the
//! examples present, their copy in `templates/vendor` (what a published crate
//! builds from) is brought up to date first.

#[path = "src/template_files.rs"]
mod template_files;

use std::env;
use std::fs;
use std::path::PathBuf;

fn main() {
    let base = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("Cargo sets it"));
    for name in template_files::EXAMPLES {
        let from = template_files::examples(&base).join(name);
        if from.is_dir() {
            let to = template_files::vendor(&base).join(name);
            if let Err(e) = template_files::refresh(&from, &to) {
                println!(
                    "cargo:warning=Could not copy {} to {}: {e}",
                    from.display(),
                    to.display()
                );
            }
        }
    }
    let mut out = String::new();
    for (name, layers) in template_files::TEMPLATES {
        out.push_str(&format!("const {name}: &[(&str, &[u8])] = &[\n"));
        // Each file, and the folders a new one may appear in; not a whole
        // example, whose builds (`.wisp`, `target`) would rerun this.
        for (dir, _) in layers {
            let root = template_files::root(dir, &base);
            for sub in ["src", "static"] {
                if root.join(sub).is_dir() {
                    println!("cargo:rerun-if-changed={}", root.join(sub).display());
                }
            }
        }
        let files = template_files::files(layers, &base).unwrap_or_else(|e| {
            eprintln!("wisp-cli: reading the {name} template: {e}");
            std::process::exit(1);
        });
        for (rel, path) in files {
            println!("cargo:rerun-if-changed={}", path.display());
            out.push_str(&format!(
                "    ({rel:?}, include_bytes!({:?})),\n",
                path.to_string_lossy()
            ));
        }
        out.push_str("];\n");
    }
    // Rewriting the same tables would rebuild the crate for nothing.
    let dest = PathBuf::from(env::var_os("OUT_DIR").expect("Cargo sets it")).join("templates.rs");
    if fs::read(&dest).ok().as_deref() != Some(out.as_bytes()) {
        fs::write(dest, out).expect("OUT_DIR is writable");
    }
}
