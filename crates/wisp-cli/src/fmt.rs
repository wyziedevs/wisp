//! `wisp fmt [paths] [--check]`: formats `.wisp` files in place, or with
//! `--check` names those that are not formatted. `wisp check` warns of them.
//! `wisp fmt --stdin [path]` formats stdin to stdout (for editors and
//! Prettier); `path` is where the text belongs, for the crate's edition.

use crate::term;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use wisp_build::fmt::{edition, format};

pub fn run(args: &[String]) -> Result<(), String> {
    let usage = "wisp fmt takes files or folders (the current one by default), --check or --stdin.";
    let mut check = false;
    let mut stdin = false;
    let mut paths = Vec::new();
    for arg in args {
        match arg.as_str() {
            "--check" => check = true,
            "--stdin" => stdin = true,
            _ if arg.starts_with('-') => return Err(format!("There is no option {arg}.\n{usage}")),
            _ => paths.push(PathBuf::from(arg)),
        }
    }
    if stdin {
        return match (check, &paths[..]) {
            (false, []) => filter(Path::new("x.wisp")),
            (false, [p]) => filter(p),
            _ => Err(format!("wisp fmt --stdin takes one path at most.\n{usage}")),
        };
    }
    // In an app only: fmt works anywhere, and --stdin is an editor's.
    if let Ok(root) = crate::app_root() {
        crate::cargo::check_updated(root)?;
    }
    if paths.is_empty() {
        paths.push(PathBuf::from("."));
    }
    let mut files = Vec::new();
    for p in &paths {
        if p.is_dir() {
            walk(p, &mut files);
        } else if p.is_file() {
            files.push(p.clone());
        } else {
            return Err(format!("There is no file or folder {}.", p.display()));
        }
    }
    let changed = format_all(&files)?;
    for (path, text) in &changed {
        if check {
            term::changed(&shown(path), "not formatted");
        } else {
            std::fs::write(path, text)
                .map_err(|e| format!("Could not write {}: {e}", shown(path)))?;
            term::changed(&shown(path), "formatted");
        }
    }
    match (check, changed.len()) {
        (_, 0) => term::done(&format!("{} files are formatted.", files.len())),
        (true, n) => {
            return Err(format!(
                "{n} of {} files are not formatted.\nRun wisp fmt.",
                files.len()
            ));
        }
        (false, n) => term::done(&format!("Formatted {n} of {} files.", files.len())),
    }
    Ok(())
}

/// Stdin formatted to stdout, as if it were the file at `path`.
fn filter(path: &Path) -> Result<(), String> {
    let mut src = String::new();
    (std::io::stdin().read_to_string(&mut src))
        .map_err(|e| format!("Could not read stdin: {e}"))?;
    let path = std::env::current_dir().map_or(path.to_path_buf(), |d| d.join(path));
    let mut out = std::io::stdout().lock();
    (out.write_all(format(&src, &edition(&path)).as_bytes()))
        .and_then(|()| out.flush())
        .map_err(|e| format!("Could not write stdout: {e}"))
}

/// For `wisp check`: the app's `.wisp` files that `wisp fmt` would change.
pub fn warn_unformatted(root: &Path) {
    let mut files = Vec::new();
    walk(&root.join("src"), &mut files);
    for (path, _) in format_all(&files).unwrap_or_default() {
        term::warn(&format!("{} is not formatted. Run wisp fmt.", shown(&path)));
    }
}

/// The `.wisp` files under `dir`, but in hidden folders, `target` and
/// `node_modules`.
fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
    entries.sort();
    for p in entries {
        let name = p
            .file_name()
            .map_or(String::new(), |n| n.to_string_lossy().into_owned());
        if p.is_dir() {
            if !name.starts_with('.') && name != "target" && name != "node_modules" {
                walk(&p, out);
            }
        } else if name.ends_with(".wisp") {
            out.push(p);
        }
    }
}

/// The files whose formatted text differs, with that text: a thread a core.
fn format_all(files: &[PathBuf]) -> Result<Vec<(PathBuf, String)>, String> {
    let threads = std::thread::available_parallelism().map_or(1, |n| n.get());
    let per = files.len().div_ceil(threads).max(1);
    let parts: Vec<Result<Vec<(PathBuf, String)>, String>> = std::thread::scope(|s| {
        let handles: Vec<_> = (files.chunks(per))
            .map(|part| {
                s.spawn(move || {
                    let mut changed = Vec::new();
                    for f in part {
                        let src = std::fs::read_to_string(f)
                            .map_err(|e| format!("Could not read {}: {e}", shown(f)))?;
                        let out = format(&src, &edition(f));
                        if out != src {
                            changed.push((f.clone(), out));
                        }
                    }
                    Ok(changed)
                })
            })
            .collect();
        (handles.into_iter())
            .map(|h| {
                h.join()
                    .unwrap_or_else(|_| Err("The formatter stopped.".into()))
            })
            .collect()
    });
    let mut changed = Vec::new();
    for p in parts {
        changed.extend(p?);
    }
    Ok(changed)
}

fn shown(p: &Path) -> String {
    let s = p.to_string_lossy().replace('\\', "/");
    s.strip_prefix("./").unwrap_or(&s).to_string()
}

#[cfg(test)]
mod tests {
    use wisp_build::fmt::format;

    /// Pieces of templates, glued in random order into odd sources.
    const BITS: [&str; 40] = [
        "<div>",
        "</div>",
        "<p class=\"a\">",
        "</p>",
        "<br>",
        "<input value={x}>",
        "{#if a}",
        "{:else}",
        "{/if}",
        "{#each xs as x}",
        "{/each}",
        "{x}",
        "{@html h}",
        "<script>",
        "let a = 1;",
        "</script>",
        "<style>",
        "p { color: red }",
        "</style>",
        "<pre>  keep  </pre>",
        "<!-- note -->",
        "text",
        " ",
        "\n",
        "\n\n\n",
        "\t",
        "é",
        "🙂",
        "<",
        ">",
        "{",
        "}",
        "\"",
        "'",
        "<textarea> a\n b </textarea>",
        "{#await p}",
        "{:then v}",
        "{/await}",
        "<span>a</span>b",
        "&amp;",
    ];

    fn squash(s: &str) -> String {
        s.chars().filter(|c| !c.is_whitespace()).collect()
    }

    /// fmt is a fixed point after one pass and never drops visible content.
    #[test]
    fn fuzz_round_trip() {
        let mut seed: u64 = 0x9e37_79b9_7f4a_7c15;
        let mut next = || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        let mut bad = Vec::new();
        for _ in 0..3000 {
            let n = next() % 24;
            let src: String = (0..n)
                .map(|_| BITS[(next() % BITS.len() as u64) as usize])
                .collect();
            let once = format(&src, "2024");
            let twice = format(&once, "2024");
            if once != twice {
                bad.push(format!(
                    "not idempotent: {src:?}\n -> {once:?}\n -> {twice:?}"
                ));
            } else if squash(&src) != squash(&once) {
                bad.push(format!("content changed: {src:?}\n -> {once:?}"));
            }
        }
        assert!(
            bad.is_empty(),
            "{} failures, first:\n{}",
            bad.len(),
            bad[..bad.len().min(4)].join("\n")
        );
    }
}
