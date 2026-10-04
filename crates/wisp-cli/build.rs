//! Writes the `wisp new` templates' file tables into OUT_DIR: one folder is
//! the one source of each, so a file added there reaches new apps. With the
//! examples present, their copy in `templates/vendor` (what a published crate
//! builds from) is brought up to date first.

#[path = "src/git_head.rs"]
mod git_head;
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
    // The AI reference apps get (`wisp new`, `wisp update-docs`, `wisp
    // mcp`): the repository's AGENTS.md, less its part for work on Wisp,
    // and llms-full.txt (that and docs/).
    let repo = template_files::repo(&base);
    // The build stamp (commits, short hash) of the checkout this CLI is built
    // from, for the check that the app's wisp is not ahead of it. Empty when
    // there is no checkout or no git.
    let (_, watched) = git_head::read(&repo).unwrap_or_default();
    for file in watched {
        println!("cargo:rerun-if-changed={}", file.display());
    }
    println!("cargo:rustc-env=WISP_CLI_STAMP={}", git_head::stamp(&repo));
    if let Ok(agents) = template_files::read_text(&repo.join("llms/AGENTS.md")) {
        println!(
            "cargo:rerun-if-changed={}",
            repo.join("llms/AGENTS.md").display()
        );
        for doc in template_files::DOCS {
            let path = repo.join(format!("docs/{doc}.md"));
            println!("cargo:rerun-if-changed={}", path.display());
        }
        let vendor = template_files::vendor(&base);
        let files = [
            ("AGENTS.md", Ok(template_files::app_agents(&agents))),
            ("llms-full.txt", template_files::llms_full(&repo)),
        ];
        for (file, text) in files {
            let to = vendor.join(file);
            if let Err(e) = text.and_then(|t| template_files::write_if_changed(&to, &t)) {
                println!("cargo:warning=Could not write {}: {e}", to.display());
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
