//! `cargo run -p wisp-tokens`: what the same five features cost in tokens in
//! each app under `apps/`, as Markdown tables.
//!
//! A file is counted when it has a `@feature NAME` line (in any comment
//! syntax); that line is not counted, and the lines after it, up to the next
//! marker, belong to NAME. Lines above a file's first marker belong to it too,
//! and so does the file's path: writing a file means naming it. Files with no
//! marker (what a generator writes: `wisp new`, `sv create`,
//! `create-next-app`, `cargo new`) are not counted, nor the lines after a
//! `@generated` marker (a `[package]` table below the `[dependencies]` one
//! has to write).
//!
//! No tokenizer is available offline, so this estimates a BPE code tokenizer
//! (cl100k-like): whitespace with a newline in it is 1 token; other spaces
//! join the word after them; a word splits at `_` and camelCase humps, and
//! each part is 1 token up to 8 letters and 1 per 6 after that; digits are 1
//! per 3; common operators are 1; any other character is 1. Characters / 4,
//! the usual rule of thumb, is printed beside it.

use std::fs;
use std::path::{Path, PathBuf};

const APPS: [(&str, &str); 5] = [
    ("wisp", "**Wisp**"),
    ("sveltekit", "SvelteKit"),
    ("nextjs", "Next.js"),
    ("axum", "Axum + askama"),
    ("actix", "Actix + tera"),
];

const FEATURES: [&str; 7] = ["list", "form", "api", "layout", "search", "data", "setup"];

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

fn main() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("apps");
    let mut rows = Vec::new();
    for (dir, name) in APPS {
        let app = root.join(dir);
        let mut files = Vec::new();
        walk(&app, &mut files);
        files.sort();
        let mut per = [Count::default(); FEATURES.len()];
        let mut counted = 0;
        for file in &files {
            let Ok(text) = fs::read_to_string(file) else {
                continue;
            };
            let rel = file
                .strip_prefix(&app)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            if count_file(&rel, &text, &mut per) {
                counted += 1;
            }
        }
        rows.push((name, per, counted));
    }

    println!("Estimated tokens by feature:\n");
    print!("| Stack |");
    for f in FEATURES {
        print!(" {f} |");
    }
    println!(" total | chars / 4 | files |");
    print!("|---|");
    for _ in 0..FEATURES.len() + 3 {
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
                "target" | "node_modules" | ".svelte-kit" | ".next" | ".wisp"
            ) {
                walk(&path, out);
            }
        } else {
            out.push(path);
        }
    }
}

/// Adds one file to `per`; false when it has no marker.
fn count_file(path: &str, text: &str, per: &mut [Count; FEATURES.len()]) -> bool {
    let mut current = None;
    let mut pending = String::new();
    let mut first = None;
    for line in text.lines() {
        let marker = if let Some(at) = line.find("@feature ") {
            let name = line[at + 9..].split_whitespace().next().unwrap_or("");
            let Some(i) = FEATURES.iter().position(|f| *f == name) else {
                panic!("{path}: unknown feature {name:?}; known: {FEATURES:?}");
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
        let mut per = [Count::default(); FEATURES.len()];
        let text = "---\n// @feature list\nlet a = 1;\n// @feature data\nb\n";
        assert!(count_file("x", text, &mut per));
        assert_eq!(per[0].tokens, tokens("---\nlet a = 1;") + tokens("x"));
        assert_eq!(per[5].tokens, 1);
        assert!(!count_file("y", "fn main() {}", &mut per));
        let mut per = [Count::default(); FEATURES.len()];
        assert!(count_file(
            "",
            "# @feature setup\na = 1\n# @generated\nb = 2\n",
            &mut per
        ));
        assert_eq!(per[6].tokens, 3);
    }
}
