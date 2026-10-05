//! `cargo run -p wisp-tokens`: what the same features cost in tokens in each
//! app of a suite, as Markdown tables: five small ones in each app under
//! `apps/`, then five of a real app (auth, CRUD, uploads, live updates, a
//! component) in each under `real/`.
//!
//! A file is counted when it has a `@feature NAME` line (in any comment
//! syntax); that line is not counted, and the lines after it, up to the next
//! marker, belong to NAME. Lines above a file's first marker belong to it too,
//! and so does the file's path: writing a file means naming it. The lines after a
//! `@generated` marker are not counted (a `[package]` table below the
//! `[dependencies]` one has to write). A file with no marker (what a generator
//! writes: `wisp new`, `sv create`, `create-next-app`, `npm create`, `npm init`,
//! `cargo new`) counts as `setup` by the lines the generator's copy of it under `scaffold/<suite>/<stack>/` lacks (every line
//! when there is no copy), so manifests and configs the author edits
//! (`package.json`, `Cargo.toml`, `nuxt.config.ts`) count alike for every
//! stack. `src/tests.rs` is not counted: no other stack's app has tests.
//!
//! No tokenizer is available offline, so this estimates a BPE code tokenizer
//! (cl100k-like): whitespace with a newline in it is 1 token; other spaces
//! join the word after them; a word splits at `_` and camelCase humps, and
//! each part is 1 token up to 8 letters and 1 per 6 after that; digits are 1
//! per 3; common operators are 1; any other character is 1. Characters / 4,
//! the usual rule of thumb, is printed beside it.

use std::fs;
use std::path::{Path, PathBuf};

/// A folder of apps, one per stack, the first Wisp's, and the features
/// their `@feature` markers may name.
struct Suite {
    heading: &'static str,
    dir: &'static str,
    apps: &'static [(&'static str, &'static str)],
    features: &'static [&'static str],
}

const SUITES: [Suite; 2] = [
    Suite {
        heading: "Estimated tokens by feature:",
        dir: "apps",
        apps: &[
            ("wisp", "**Wisp**"),
            ("sveltekit", "SvelteKit"),
            ("nextjs", "Next.js"),
            ("nuxt", "Nuxt (Vue)"),
            ("react", "React (Vite + Express)"),
            ("express", "Express (Node.js)"),
            ("axum", "Axum + askama"),
            ("actix", "Actix + tera"),
        ],
        features: &["list", "form", "api", "layout", "search", "data", "setup"],
    },
    Suite {
        heading: "Estimated tokens by feature, a real app (bench/tokens/real):",
        dir: "real",
        apps: &[
            ("wisp", "**Wisp**"),
            ("sveltekit", "SvelteKit"),
            ("nextjs", "Next.js"),
        ],
        features: &[
            "auth",
            "crud",
            "upload",
            "live",
            "component",
            "data",
            "setup",
        ],
    },
];

const OPERATORS: [&str; 28] = [
    "<!--", "-->", "===", "!==", "...", "..=", "::", "->", "=>", "==", "!=", "</", "/>", "{{",
    "}}", "{%", "%}", "<=", ">=", "&&", "||", "..", "+=", "++", "//", "/*", "*/", "${",
];

/// A `@generated` line: what follows, up to the next marker, is not counted.
const GENERATED: usize = usize::MAX;

#[derive(Default, Clone, Copy)]
struct Count {
    tokens: usize,
    chars: usize,
}

/// Prints each suite's table and writes the totals to `results.json`
/// (`{"<dir>": [{"stack", "tokens", "files"}..]}`), which the docs site reads.
fn main() {
    let mut json = Vec::new();
    for (i, suite) in SUITES.iter().enumerate() {
        if i > 0 {
            println!();
        }
        let rows: Vec<String> = table(suite)
            .iter()
            .map(|(name, tokens, files)| {
                let name = name.trim_matches('*');
                format!("{{\"stack\": \"{name}\", \"tokens\": {tokens}, \"files\": {files}}}")
            })
            .collect();
        json.push(format!(
            "  \"{}\": [
    {}
  ]",
            suite.dir,
            rows.join(
                ",
    "
            )
        ));
    }
    let out = Path::new(env!("CARGO_MANIFEST_DIR")).join("results.json");
    let _ = fs::write(
        out,
        format!(
            "{{
{}
}}
",
            json.join(
                ",
"
            )
        ),
    );
}

/// Prints a suite's table; returns each stack's (name, tokens, files counted).
fn table(suite: &Suite) -> Vec<(&'static str, usize, usize)> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join(suite.dir);
    let features = suite.features;
    let scaffold = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("scaffold")
        .join(suite.dir);
    let (mut rows, mut lists) = (Vec::new(), Vec::new());
    for &(dir, name) in suite.apps {
        let app = root.join(dir);
        let mut files = Vec::new();
        walk(&app, &mut files);
        files.sort();
        let mut per = vec![Count::default(); features.len()];
        let (mut counted, mut listed, mut skipped) = (0, Vec::new(), Vec::new());
        for file in &files {
            let Ok(text) = fs::read_to_string(file) else {
                continue;
            };
            let rel = file
                .strip_prefix(&app)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            // Tests are no feature: no other stack's app has any.
            let counts = rel != "src/tests.rs"
                && (count_file(&rel, &text, features, &mut per)
                    || count_unmarked(&scaffold.join(dir).join(&rel), &text, features, &mut per));
            if counts {
                counted += 1;
                listed.push(rel);
            } else {
                skipped.push(rel);
            }
        }
        lists.push((name, listed, skipped));
        rows.push((name, per, counted));
    }

    println!("{}\n", suite.heading);
    print!("| Stack |");
    for f in features {
        print!(" {f} |");
    }
    println!(" total | chars / 4 | files |");
    print!("|---|");
    for _ in 0..features.len() + 3 {
        print!("---:|");
    }
    println!();
    for (name, per, files) in &rows {
        let total: usize = per.iter().map(|c| c.tokens).sum();
        let chars: usize = per.iter().map(|c| c.chars).sum();
        print!("| {name} |");
        for c in per {
            print!(" {} |", c.tokens);
        }
        println!(" {total} | {} | {files} |", chars / 4);
    }
    let wisp: usize = rows[0].1.iter().map(|c| c.tokens).sum();
    println!();
    for (name, per, _) in &rows[1..] {
        let total: usize = per.iter().map(|c| c.tokens).sum();
        println!("{name}: {:.1}x Wisp", total as f64 / wisp as f64);
    }
    // Which files each stack counted (and which it skipped for want of a marker), for auditing.
    println!();
    for (name, listed, skipped) in &lists {
        println!("{name} counted: {}", listed.join(", "));
        println!("{name} not counted: {}", skipped.join(", "));
    }
    rows.iter()
        .map(|(name, per, files)| (*name, per.iter().map(|c| c.tokens).sum(), *files))
        .collect()
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if path.is_dir() {
            if !matches!(
                &*name,
                "target"
                    | "node_modules"
                    | ".svelte-kit"
                    | ".next"
                    | ".nuxt"
                    | ".output"
                    | "dist"
                    | ".wisp"
            ) {
                walk(&path, out);
            }
        } else if !matches!(
            // Lockfiles are written by the package manager, never by hand.
            &*name,
            "package-lock.json"
                | "pnpm-lock.yaml"
                | "yarn.lock"
                | "bun.lock"
                | "bun.lockb"
                | "Cargo.lock"
        ) {
            out.push(path);
        }
    }
}

/// Adds one file to `per`, a count for each of `features`; false when it
/// has no marker.
fn count_file(path: &str, text: &str, features: &[&str], per: &mut [Count]) -> bool {
    let mut current = None;
    let mut pending = String::new();
    let mut first = None;
    for line in text.lines() {
        let marker = if let Some(at) = line.find("@feature ") {
            let name = line[at + 9..].split_whitespace().next().unwrap_or("");
            let Some(i) = features.iter().position(|f| *f == name) else {
                panic!("{path}: unknown feature {name:?}; known: {features:?}");
            };
            first.get_or_insert(i);
            Some(i)
        } else if line.contains("@generated") {
            Some(GENERATED)
        } else {
            None
        };
        let Some(next) = marker else {
            pending.push_str(line);
            pending.push('\n');
            continue;
        };
        // Lines above a file's first marker are its first feature's.
        if let Some(c) = current {
            if c != GENERATED {
                add(&mut per[c], &pending);
            }
            pending.clear();
        }
        current = Some(next);
    }
    let Some(first) = first else { return false };
    if let Some(c) = current.filter(|&c| c != GENERATED) {
        add(&mut per[c], &pending);
    }
    add(&mut per[first], path);
    true
}

/// A file with no marker (a manifest or config the stack's generator writes):
/// its lines that the generator's copy under `scaffold/` lacks are what the
/// author typed, and count as `setup`; with no copy, every line counts. False
/// when nothing is left.
fn count_unmarked(scaffold: &Path, text: &str, features: &[&str], per: &mut [Count]) -> bool {
    let key = |l: &str| l.trim().trim_end_matches(',').to_string();
    let base: Vec<String> = fs::read_to_string(scaffold)
        .unwrap_or_default()
        .lines()
        .map(key)
        .collect();
    let typed: Vec<&str> = text
        .lines()
        .filter(|l| !l.trim().is_empty() && !base.contains(&key(l)))
        .collect();
    if typed.is_empty() {
        return false;
    }
    let setup = features
        .iter()
        .position(|f| *f == "setup")
        .expect("a setup feature");
    add(
        &mut per[setup],
        &typed.join(
            "
",
        ),
    );
    true
}

fn add(c: &mut Count, text: &str) {
    let text = text.trim();
    c.tokens += tokens(text);
    c.chars += text.chars().count();
}

/// The estimate described at the top.
fn tokens(text: &str) -> usize {
    let b: Vec<char> = text.chars().collect();
    let mut n = 0;
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        if c.is_whitespace() {
            let start = i;
            while i < b.len() && b[i].is_whitespace() {
                i += 1;
            }
            if b[start..i].contains(&'\n') {
                n += 1;
            }
        } else if c.is_ascii_alphabetic() || c == '_' {
            let start = i;
            while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == '_') {
                i += 1;
            }
            n += word(&b[start..i]);
        } else if c.is_ascii_digit() {
            let start = i;
            while i < b.len() && b[i].is_ascii_digit() {
                i += 1;
            }
            n += (i - start).div_ceil(3);
        } else {
            let rest: String = b[i..b.len().min(i + 4)].iter().collect();
            let op = OPERATORS.iter().find(|op| rest.starts_with(**op));
            i += op.map_or(1, |op| op.len());
            n += 1;
        }
    }
    n
}

/// Parts split at `_` and lower-to-upper humps.
fn word(w: &[char]) -> usize {
    let mut n = 0;
    let mut len = 0;
    for (k, &c) in w.iter().enumerate() {
        let hump = k > 0 && c.is_ascii_uppercase() && w[k - 1].is_ascii_lowercase();
        if c == '_' || hump {
            n += part(len);
            len = usize::from(c != '_');
        } else {
            len += 1;
        }
    }
    n + part(len)
}

fn part(len: usize) -> usize {
    match len {
        0 => 0,
        1..=8 => 1,
        _ => 1 + (len - 8).div_ceil(6),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn estimate() {
        assert_eq!(tokens("let x = 1;"), 5);
        assert_eq!(tokens("fn get() -> Vec<Item>"), 9);
        assert_eq!(tokens("a\n    b"), 3);
        assert_eq!(tokens("toLowerCase snake_case"), 5);
        assert_eq!(tokens("1234567"), 3);
        assert_eq!(tokens("</a>"), 3);
    }

    #[test]
    fn markers() {
        let features = SUITES[0].features;
        let mut per = vec![Count::default(); features.len()];
        let text = "---\n// @feature list\nlet a = 1;\n// @feature data\nb\n";
        assert!(count_file("x", text, features, &mut per));
        assert_eq!(per[0].tokens, tokens("---\nlet a = 1;") + tokens("x"));
        assert_eq!(per[5].tokens, 1);
        assert!(!count_file("y", "fn main() {}", features, &mut per));
        let mut per = vec![Count::default(); features.len()];
        assert!(count_file(
            "",
            "# @feature setup\na = 1\n# @generated\nb = 2\n",
            features,
            &mut per
        ));
        assert_eq!(per[6].tokens, 3);
    }
}
