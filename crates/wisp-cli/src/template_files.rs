//! The files of each `wisp new` template: build.rs embeds them, and a test
//! checks what it embedded.
//!
//! The examples are the one source. A published crate has no `examples`
//! folder beside it, so build.rs copies them into `templates/vendor` (which
//! is packaged) whenever they are there and differ, and a build without
//! them reads that copy.

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// The examples some templates are made of, by folder name.
pub const EXAMPLES: [&str; 3] = ["demo", "api", "axum"];

/// A folder, from this crate's (or an example, by name), and which of its
/// files (paths inside it, with `/`) the template takes.
pub type Layer = (&'static str, fn(&str) -> bool);

/// A template is its layers; a later layer's file replaces an earlier one at
/// the same path. The demo and the API are the examples of those names, so
/// the two can never drift. The minimal template is the demo's shell (main,
/// build script, app.html, static files) in the axum example's styles, with
/// pages of its own (and none of the demo's tests of its pages).
pub const TEMPLATES: [(&str, &[Layer]); 3] = [
    ("DEMO", &[("demo", all)]),
    ("API", &[("api", all)]),
    (
        "MINIMAL",
        &[
            ("demo", not_pages),
            ("axum", only_css),
            ("templates/minimal", all),
        ],
    ),
];

fn all(_: &str) -> bool {
    true
}

fn not_pages(path: &str) -> bool {
    !path.starts_with("src/routes/") && path != "src/app.css" && path != "src/tests.rs"
}

fn only_css(path: &str) -> bool {
    path == "src/app.css"
}

/// Where a layer's files are: the example when it is there, else its copy
/// in this crate.
pub fn root(dir: &str, base: &Path) -> PathBuf {
    if !EXAMPLES.contains(&dir) {
        return base.join(dir);
    }
    let example = examples(base).join(dir);
    if example.is_dir() {
        example
    } else {
        vendor(base).join(dir)
    }
}

pub fn examples(base: &Path) -> PathBuf {
    base.join("../../examples")
}

pub fn vendor(base: &Path) -> PathBuf {
    base.join("templates/vendor")
}

/// Makes `to` hold the files of `from` and no others, writing only what
/// differs: a build that changes nothing does not touch the source tree.
pub fn refresh(from: &Path, to: &Path) -> io::Result<()> {
    let want = read_all(from)?;
    let have = read_all(to)?;
    for (rel, bytes) in &want {
        if have.get(rel) != Some(bytes) {
            let path = to.join(rel);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::write(path, bytes)?;
        }
    }
    for rel in have.keys().filter(|rel| !want.contains_key(*rel)) {
        fs::remove_file(to.join(rel))?;
    }
    Ok(())
}

/// A folder's template files and their contents; none if it is not there.
pub fn read_all(dir: &Path) -> io::Result<BTreeMap<String, Vec<u8>>> {
    let mut paths = Vec::new();
    if dir.is_dir() {
        walk(dir, "", &mut paths)?;
    }
    let mut all = BTreeMap::new();
    for rel in paths {
        let bytes = fs::read(dir.join(&rel))?;
        all.insert(rel, bytes);
    }
    Ok(all)
}

/// The template's files, by path in the app, sorted. Not in a template: what
/// a new app makes itself (Cargo.toml, .gitignore), what is built or
/// downloaded (target, node_modules, dot folders, data), and tests.
pub fn files(layers: &[Layer], base: &Path) -> io::Result<BTreeMap<String, PathBuf>> {
    let mut found = BTreeMap::new();
    for (dir, keep) in layers {
        let root = root(dir, base);
        let mut paths = Vec::new();
        walk(&root, "", &mut paths)?;
        for rel in paths.into_iter().filter(|p| keep(p)) {
            found.insert(rel.clone(), root.join(&rel));
        }
    }
    Ok(found)
}

fn walk(dir: &Path, rel: &str, out: &mut Vec<String>) -> io::Result<()> {
    let at = |e: io::Error| io::Error::new(e.kind(), format!("{}: {e}", dir.display()));
    for entry in fs::read_dir(dir).map_err(at)? {
        let entry = entry.map_err(at)?;
        let name = entry.file_name().to_string_lossy().into_owned();
        let top = rel.is_empty();
        let skip = name.starts_with('.')
            || name == "node_modules"
            || top
                && [
                    "Cargo.toml",
                    "Cargo.lock",
                    "openapi.json",
                    "tests",
                    "target",
                    "data",
                ]
                .contains(&name.as_str());
        if skip {
            continue;
        }
        let path = if top { name } else { format!("{rel}/{name}") };
        if entry.file_type().is_ok_and(|t| t.is_dir()) {
            walk(&entry.path(), &path, out)?;
        } else {
            out.push(path);
        }
    }
    Ok(())
}

/// The repository's root, where AGENTS.md, docs and llms-full.txt are.
pub fn repo(base: &Path) -> PathBuf {
    base.join("../..")
}

/// The docs `llms-full.txt` holds after AGENTS.md, in order.
pub const DOCS: [&str; 4] = ["client", "api", "deploy", "embed"];

/// A text file with `\r\n` as `\n`, as a checkout on Windows may have it.
pub fn read_text(path: &Path) -> io::Result<String> {
    Ok(fs::read_to_string(path)?.replace("\r\n", "\n"))
}

/// The app's AGENTS.md: the repository's, less what is between
/// `<!-- repo` and `<!-- /repo -->` (rules for work on Wisp itself), and
/// the line after which the app's own notes go (`new::END`).
pub fn app_agents(text: &str) -> String {
    let mut out = reference(text);
    out.push_str("\n<!-- End of the Wisp reference. Notes for this app go below; wisp update-docs keeps them. -->\n");
    out
}

fn reference(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut keep = true;
    for line in text.split_inclusive('\n') {
        if line.starts_with("<!-- repo") {
            keep = false;
        } else if line.starts_with("<!-- /repo") {
            keep = true;
        } else if keep {
            out.push_str(line);
        }
    }
    out
}

/// `llms-full.txt`: the app's AGENTS.md, then each of [`DOCS`], one file.
pub fn llms_full(repo: &Path) -> io::Result<String> {
    let mut out = reference(&read_text(&repo.join("llms/AGENTS.md"))?);
    for doc in DOCS {
        out.push_str(&format!("\n\n<!-- docs/{doc}.md -->\n\n"));
        out.push_str(read_text(&repo.join(format!("docs/{doc}.md")))?.trim_end());
        out.push('\n');
    }
    Ok(out)
}

/// Writes `text` to `path` unless it is there already.
pub fn write_if_changed(path: &Path, text: &str) -> io::Result<()> {
    if fs::read(path).ok().as_deref() == Some(text.as_bytes()) {
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, text)
}
