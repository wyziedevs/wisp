//! The build on broken input: every prefix of the real templates and Rust
//! files in this repository, and many random edits of them, must give an
//! answer (code or an error), never a panic.

use std::fs;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};

/// The `.wisp` and `.rs` files of the test app and the examples.
fn corpus(ext: &str) -> Vec<(PathBuf, String)> {
    let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut out = Vec::new();
    let mut dirs = vec![repo.join("tests/app/src"), repo.join("examples")];
    while let Some(d) = dirs.pop() {
        let Ok(rd) = fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                if !p.ends_with("target") && !p.ends_with("node_modules") {
                    dirs.push(p);
                }
            } else if p.extension().is_some_and(|x| x == ext)
                && let Ok(s) = wisp_build::read_source(&p)
            {
                out.push((p, s));
            }
        }
    }
    out.sort();
    assert!(!out.is_empty(), "no .{ext} files found");
    out
}

/// How many times over to run the random tests: `WISP_FUZZ=50` for a long
/// hunt (with its seed from `WISP_FUZZ_SEED`).
fn rounds() -> usize {
    std::env::var("WISP_FUZZ")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(1)
}

fn seed(n: u64) -> u64 {
    let s = std::env::var("WISP_FUZZ_SEED")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0u64);
    n ^ s.wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1
}

/// The same edits on every run.
use wisp_shared::rng::Rng;

const BITS: &[&str] = &[
    "{",
    "}",
    "<",
    ">",
    "\"",
    "'",
    "/",
    "*",
    "-",
    "#",
    "\n",
    "---\n",
    "{#if x}",
    "{/if}",
    "{:else}",
    "{#each a as b}",
    "{/each}",
    "{@html",
    "{@const",
    "<script>",
    "</script>",
    "<style>",
    "<!--",
    "-->",
    "r#\"",
    "\"#",
    "b'",
    "\\",
    "é",
    "\u{2028}",
    "ß",
    "::",
    "?",
    ".await",
    "#[action]",
    "fn ",
    "(",
    ")",
    "[",
    "]",
    ";",
    "=",
    "{:",
    "on:click=",
    "bind:value=",
    "<Card",
    "/>",
    "`",
    "${",
    "|",
    "&",
    "\0",
];

/// `s` with a few random cuts, copies and insertions, on char boundaries.
fn mutate(s: &str, rng: &mut Rng) -> String {
    let mut s = s.to_string();
    for _ in 0..1 + rng.below(4) {
        let mut at = rng.below(s.len() + 1);
        while !s.is_char_boundary(at) {
            at -= 1;
        }
        match rng.below(3) {
            0 => {
                let mut end = (at + rng.below(12)).min(s.len());
                while !s.is_char_boundary(end) {
                    end -= 1;
                }
                s.replace_range(at..end, "");
            }
            1 => s.insert_str(at, BITS[rng.below(BITS.len())]),
            _ => {
                let mut from = rng.below(s.len() + 1);
                while !s.is_char_boundary(from) {
                    from -= 1;
                }
                let mut end = (from + rng.below(40)).min(s.len());
                while !s.is_char_boundary(end) {
                    end -= 1;
                }
                let piece = s[from..end].to_string();
                s.insert_str(at, &piece);
            }
        }
    }
    s
}

/// The last panic's message and place.
static WHY: std::sync::Mutex<String> = std::sync::Mutex::new(String::new());

/// Runs `f` on `input`, keeping the input (and where it came from) when
/// it panicked.
fn survives(bad: &mut Vec<String>, what: &str, input: &str, f: impl Fn(&str)) {
    let panics = |s: &str| catch_unwind(AssertUnwindSafe(|| f(s))).is_err();
    if bad.len() < 5 && panics(input) {
        let why = WHY.lock().unwrap_or_else(|e| e.into_inner()).clone();
        let at = why.lines().next().unwrap_or("").to_string();
        // Shrunk while it still panics at the same place, so the report is
        // the few bytes that matter.
        let same = |s: &str| {
            panics(s)
                && WHY
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .starts_with(&at)
        };
        let mut s = input.to_string();
        let mut cut = s.len() / 2;
        while cut > 0 {
            let mut at = 0;
            while at < s.len() {
                let mut end = (at + cut).min(s.len());
                while !s.is_char_boundary(end) {
                    end += 1;
                }
                let mut t = s.clone();
                t.replace_range(at..end, "");
                if same(&t) {
                    s = t;
                } else {
                    at = end;
                }
            }
            cut /= 2;
        }
        bad.push(format!("{what}: {why}\n----\n{s}\n----"));
    }
}

/// Silences panics while `f` runs, then fails with the inputs that made
/// one. The tests share the panic hook, so they take turns.
fn quietly(f: impl FnOnce(&mut Vec<String>)) {
    static TURN: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _turn = TURN.lock().unwrap_or_else(|e| e.into_inner());
    let mut bad = Vec::new();
    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(|info| {
        *WHY.lock().unwrap_or_else(|e| e.into_inner()) = info.to_string();
    }));
    f(&mut bad);
    std::panic::set_hook(hook);
    assert!(bad.is_empty(), "panicked on:\n{}", bad.join("\n"));
}

/// The fast, in-memory parts: the template parser and the Rust scanner.
fn parse_all(src: &str) {
    let _ = wisp_build::parse_wisp(src);
    let _ = wisp_build::template::parse(src);
    let _ = wisp_build::rust_scan::scan(src);
    let (items, stmts) = wisp_build::rust_scan::split_items(src);
    let _ = wisp_build::rust_scan::let_names(&stmts);
    let _ = wisp_build::rust_scan::scan(&items);
    let _ = wisp_build::rust_scan::awaits(src);
    let _ = wisp_build::rust_scan::line_ends_in_code(src);
    let _ = wisp_build::rust_scan::inner_end(src);
}

#[test]
fn every_prefix_parses_or_errs() {
    quietly(|bad| {
        for ext in ["wisp", "rs"] {
            for (path, src) in corpus(ext) {
                let what = path.display().to_string();
                for (at, _) in src.char_indices() {
                    survives(bad, &what, &src[..at], parse_all);
                }
            }
        }
    });
}

#[test]
fn random_edits_parse_or_err() {
    let mut rng = Rng(seed(0x9e37_79b9_7f4a_7c15));
    quietly(|bad| {
        for ext in ["wisp", "rs"] {
            for (path, src) in corpus(ext) {
                let what = path.display().to_string();
                for _ in 0..300 * rounds() {
                    survives(bad, &what, &mutate(&src, &mut rng), parse_all);
                }
            }
        }
    });
}

/// The whole build (routes, codegen, the client script compiler) on
/// files made from random edits of the real ones: as a page, a layout, a
/// `+server.rs`, `src/hooks.rs`, and `src/app.html`.
#[test]
fn random_projects_build_or_err() {
    let mut rng = Rng(seed(0x2545_f491_4f6c_dd1d));
    let shell = "<!doctype html>
<html><head>%wisp.head%</head>
<body>%wisp.body%</body></html>
";
    let (wisp, rs) = (corpus("wisp"), corpus("rs"));
    let root = std::env::temp_dir().join(format!("wisp-build-fuzz-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    for d in ["src/routes/x", "src/routes/y", "src/components"] {
        fs::create_dir_all(root.join(d)).unwrap();
    }
    let card = "{@props title: &str = \"\"}<b>{title}</b>{@render children()}";
    fs::write(root.join("src/components/Card.wisp"), card).unwrap();
    fs::write(root.join("src/routes/x/+page.wisp"), "<p>x</p>").unwrap();
    quietly(|bad| {
        let mut each = |what: &str, file: &str, src: &str, rng: &mut Rng| {
            let path = root.join(file);
            for n in 0..10 * rounds() {
                let text = if n == 0 {
                    src.to_string()
                } else {
                    mutate(src, rng)
                };
                survives(bad, what, &text, |s| {
                    let _ = fs::write(&path, s);
                    let _ = wisp_build::check(&root);
                });
            }
            let _ = fs::remove_file(&path);
        };
        for (path, src) in &wisp {
            let what = path.display().to_string();
            each(&what, "src/routes/y/+page.wisp", src, &mut rng);
            each(&what, "src/routes/+layout.wisp", src, &mut rng);
        }
        for (path, src) in &rs {
            let what = path.display().to_string();
            each(&what, "src/routes/y/+server.rs", src, &mut rng);
            each(&what, "src/hooks.rs", src, &mut rng);
        }
        for _ in 0..20 {
            each("app.html", "src/app.html", shell, &mut rng);
        }
    });
    let _ = fs::remove_dir_all(&root);
}
