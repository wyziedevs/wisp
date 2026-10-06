//! Every user-facing name, taken from the source, is on the docs site: CLI
//! commands and flags, `const` knobs, environment variables, the prelude and
//! `wisp::` items, `Cx`/`Response`/`Table`/`Row` methods, the browser
//! runtime's exports and the editor hover tables. Skips without the docs
//! checkout (`WISP_DOCS_DIR`, default `../wisp-docs`).

use crate::template_files::{docs_files, read_text, repo};
use std::path::{Path, PathBuf};

/// Every environment variable Wisp reads, and whether an author sets it.
/// A read the scan finds that is not here fails the test: classify it.
const ENV: [(&str, bool); 109] = [
    ("AWS_LAMBDA_RUNTIME_API", true),
    ("CRON_SECRET", true),
    ("EDITOR", true),
    ("HOST", true),
    ("METRICS_KEY", true),
    ("NO_COLOR", true),
    ("OTEL_BSP_SCHEDULE_DELAY", true),
    ("OTEL_EXPORTER_OTLP_ENDPOINT", true),
    ("OTEL_EXPORTER_OTLP_HEADERS", true),
    ("OTEL_EXPORTER_OTLP_TRACES_ENDPOINT", true),
    ("OTEL_EXPORTER_OTLP_TRACES_HEADERS", true),
    ("OTEL_SERVICE_NAME", true),
    ("PORT", true),
    ("SITE_TITLE", true),
    ("SITE_URL", true),
    ("WISP_API_DOCS", true),
    ("WISP_BASE", true),
    ("WISP_BODY_LIMIT", true),
    ("WISP_BROWSER", true),
    ("WISP_CLIENT_IP_HEADER", true),
    ("WISP_CWEBP", true),
    ("WISP_DATA", true),
    ("WISP_DEV", true),
    ("WISP_DEV_CARGO", true),
    ("WISP_DEV_DIRECT", true),
    ("WISP_EDITOR", true),
    ("WISP_FSYNC", true),
    ("WISP_HANDLER_TIMEOUT", true),
    ("WISP_HSTS", true),
    ("WISP_IO", true),
    ("WISP_LOG", true),
    ("WISP_MAX_CONNS", true),
    ("WISP_NO_UPDATE_CHECK", true),
    ("WISP_PORT_TRIES", true),
    ("WISP_PROBLEM_JSON", true),
    ("WISP_REQUEST_ID", true),
    ("WISP_NODE_HTTP", true),
    ("WISP_SASS", true),
    ("WISP_SECRET", true),
    ("WISP_SECRET_OLD", true),
    ("WISP_SECURE_HEADERS", true),
    ("WISP_SERVER_TIMING", true),
    ("WISP_STORE", true),
    ("WISP_STORE_POLL", true),
    ("WISP_STORE_TOKEN", true),
    ("WISP_THREADS", true),
    ("WISP_TSC", true),
    ("WISP_WS_IDLE", true),
    // Set by the CLI or a host bridge for the app it builds or runs.
    ("WISP_CLI", false),
    ("WISP_CLI_STAMP", false),
    ("WISP_DEV_EVENTS", false),
    ("WISP_DEV_ORIGIN", false),
    ("WISP_EXPORT", false),
    ("WISP_JOBS", false),
    ("WISP_PRERENDER", false),
    ("WISP_PRERENDERED", false),
    ("WISP_REQUEST_ONLY", false),
    ("WISP_RUNTIME_V", false),
    ("WISP_RUSTC_BIN", false),
    ("WISP_RUSTC_RECORD", false),
    ("WISP_SOURCEMAP", false),
    ("WISP_SPA", false),
    ("WISP_TAILWIND", false),
    ("WISP_TYPES", false),
    ("WISP_WS", false),
    ("WISP_WARM_UP", false),
    // Wisp's own tests and tooling.
    ("WISP_CONF_UNSET", false),
    ("WISP_DOCS_DIR", false),
    ("WISP_FUZZ", false),
    ("WISP_FUZZ_SEED", false),
    ("WISP_SKIP_LOG", false),
    ("WISP_SURELY_NOT_SET", false),
    ("WISP_TEST_NOWHERE", false),
    ("WISP_TEST_ONLY_IN_FILE", false),
    ("WISP_UNSET_PROBE_CLIENT_ID", false),
    ("WISP_UNSET_SECRET", false),
    ("WISP_BUILD_TEST_CHILD", false),
    ("API_KEY", false),
    ("WORKERS", false),
    ("ID", false),
    ("SECRET", false),
    // The platform, cargo and the terminal.
    ("CARGO", false),
    ("CARGO_BIN_NAME", false),
    ("CARGO_CFG_DEBUG_ASSERTIONS", false),
    ("CARGO_CRATE_NAME", false),
    ("CARGO_HOME", false),
    ("CARGO_MANIFEST_DIR", false),
    ("CARGO_NET_OFFLINE", false),
    ("CARGO_PKG_NAME", false),
    ("CARGO_PKG_VERSION", false),
    ("CARGO_PKG_VERSION_MAJOR", false),
    ("CARGO_PRIMARY_PACKAGE", false),
    ("CARGO_PROFILE_DEV_OPT_LEVEL", false),
    ("CARGO_PROFILE_RELEASE_CODEGEN_UNITS", false),
    ("CARGO_PROFILE_RELEASE_LTO", false),
    ("CARGO_PROFILE_RELEASE_OPT_LEVEL", false),
    ("CARGO_TARGET_DIR", false),
    ("CARGO_TARGET_TMPDIR", false),
    ("CI", false),
    ("DENO_DEPLOYMENT_ID", false),
    ("HOME", false),
    ("OUT_DIR", false),
    ("PATH", false),
    ("PROFILE", false),
    ("RUST_BACKTRACE", false),
    ("TERM", false),
    ("TERM_PROGRAM", false),
    ("USERPROFILE", false),
    ("WT_SESSION", false),
];

fn rs_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    let mut all: Vec<PathBuf> = rd.filter_map(|e| e.ok().map(|e| e.path())).collect();
    all.sort();
    for p in all {
        if p.is_dir() {
            if !p.ends_with("vendor") {
                rs_files(&p, out);
            }
        } else if p.extension().is_some_and(|e| e == "rs") {
            out.push(p);
        }
    }
}

fn ident(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

/// `name` stands in `docs` as a word of its own.
fn mentions(docs: &str, name: &str) -> bool {
    docs.match_indices(name).any(|(i, _)| {
        let before = docs[..i].chars().next_back().is_none_or(|c| !ident(c));
        let after = docs[i + name.len()..]
            .chars()
            .next()
            .is_none_or(|c| !ident(c));
        before && after
    })
}

/// The string literal right after each `pre` in `text` (`pre` ends at `"`).
fn literals<'a>(text: &'a str, pre: &str) -> Vec<&'a str> {
    text.match_indices(pre)
        .filter_map(|(i, _)| {
            let rest = &text[i + pre.len()..];
            rest.find('"').map(|end| &rest[..end])
        })
        .collect()
}

/// The environment variables the source reads: literals passed to the
/// readers, every `WISP_*`/`OTEL_*` literal, and the bridges' `env.X`.
fn env_reads(files: &[PathBuf], bridges: &Path) -> Vec<String> {
    let name = |s: &str| {
        !s.is_empty()
            && s.chars()
                .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
            && s.starts_with(|c: char| c.is_ascii_uppercase())
    };
    let mut out = Vec::new();
    for f in files {
        let text = read_text(f).unwrap_or_default();
        for pre in ["var(\"", "var_os(\"", "env(\"", "env_or(\"", "env!(\""] {
            out.extend(
                literals(&text, pre)
                    .into_iter()
                    .filter(|s| name(s))
                    .map(String::from),
            );
        }
        for pre in ["\"WISP_", "\"OTEL_"] {
            for s in literals(&text, pre) {
                let full = format!("{}{s}", &pre[1..]);
                if name(&full) && !full.ends_with('_') {
                    out.push(full);
                }
            }
        }
    }
    // The host bridges of `wisp build --target` (JavaScript).
    if let Ok(rd) = std::fs::read_dir(bridges) {
        let mut paths: Vec<PathBuf> = rd.filter_map(|e| e.ok().map(|e| e.path())).collect();
        paths.sort();
        for p in paths
            .iter()
            .filter(|p| p.extension().is_some_and(|e| e != "rs"))
        {
            let text = read_text(p).unwrap_or_default();
            for pre in ["env.get('", "env['", "env."] {
                for (i, _) in text.match_indices(pre) {
                    let rest = &text[i + pre.len()..];
                    let n = &rest[..rest.find(|c| !ident(c)).unwrap_or(rest.len())];
                    if name(n) {
                        out.push(n.to_string());
                    }
                }
            }
        }
    }
    out.sort();
    out.dedup();
    out
}

/// Names `lib.rs` makes public at the crate root: `pub mod`, `pub use` and
/// `pub fn` items, less the ones under `#[doc(hidden)]`.
fn root_names(lib: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut hidden = false;
    let mut uses = String::new();
    let mut in_use = false;
    for line in lib.lines() {
        if in_use {
            uses.push_str(line);
            in_use = !line.contains(';');
            continue;
        }
        if line.starts_with("#[doc(hidden)]") {
            hidden = true;
            continue;
        }
        if line.starts_with("#[") || line.starts_with("///") {
            continue;
        }
        let was_hidden = std::mem::take(&mut hidden);
        let item = |pre: &str| {
            line.strip_prefix(pre)
                .map(|r| r.split(|c| !ident(c)).next().unwrap_or(""))
        };
        if let Some(n) = item("pub mod ")
            .or_else(|| item("pub fn "))
            .or_else(|| item("pub async fn "))
        {
            if !was_hidden && n != "prelude" {
                out.push(n.to_string());
            }
        } else if line.starts_with("pub use ") {
            uses.push_str(line);
            in_use = !line.contains(';');
        }
        if !in_use && !uses.is_empty() {
            let list = match uses.split_once('{') {
                Some((_, group)) => group,
                None => uses.rsplit("::").next().unwrap_or(""),
            };
            for n in list.split(',') {
                let n = n.trim().trim_end_matches([';', '}', ' ']);
                let n = n.rsplit("::").next().unwrap_or("").trim();
                if !n.is_empty() {
                    out.push(n.to_string());
                }
            }
            uses.clear();
        }
    }
    out.sort();
    out.dedup();
    out
}

/// `pub fn` methods of the `impl` blocks of `text` whose header starts with
/// one of `heads`.
fn methods(text: &str, heads: &[&str]) -> Vec<String> {
    let mut out = Vec::new();
    let mut inside = false;
    let mut hidden = false;
    for line in text.lines() {
        if line.starts_with("impl") {
            inside = heads.iter().any(|h| line.starts_with(h));
        } else if line.starts_with('}') {
            inside = false;
        } else if inside {
            let l = line.trim_start();
            if l.starts_with("#[doc(hidden)]") {
                hidden = true;
                continue;
            }
            if l.starts_with("#[") || l.starts_with("///") {
                continue;
            }
            let was_hidden = std::mem::take(&mut hidden);
            let indent = line.len() - l.len() == 4;
            let rest = ["pub fn ", "pub async fn ", "pub const fn "]
                .iter()
                .find_map(|p| l.strip_prefix(p));
            if let (true, false, Some(r)) = (indent, was_hidden, rest) {
                out.push(r.split(|c| !ident(c)).next().unwrap_or("").to_string());
            }
        }
    }
    out
}

#[test]
fn docs_cover_the_surface() {
    let base = Path::new(env!("CARGO_MANIFEST_DIR"));
    let root = repo(base);
    let pages = docs_files(&root);
    if pages.is_empty() {
        let why = "docs_cover_the_surface: no docs checkout (WISP_DOCS_DIR)";
        eprintln!("skipped: {why}");
        if let Some(log) = std::env::var_os("WISP_SKIP_LOG") {
            use std::io::Write;
            let f = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(log);
            let _ = f.and_then(|mut f| writeln!(f, "  wisp-web: {why}"));
        }
        return;
    }
    let docs: String = pages
        .iter()
        .map(|(_, p)| read_text(p).unwrap_or_default())
        .collect();
    let src = |p: &str| read_text(&root.join(p)).unwrap_or_default();
    let mut missing: Vec<String> = Vec::new();
    let mut need = |kind: &str, name: &str, found: bool| {
        if !found {
            missing.push(format!("{kind} {name}"));
        }
    };

    // CLI: `wisp <command> [<sub>]` and every flag of the help tables.
    for (usage, _) in crate::COMMANDS.iter().chain(&crate::NEW_OPTIONS) {
        let words: Vec<&str> = usage
            .split([' ', '|', '[', ']', ',', '='])
            .filter(|w| !w.is_empty())
            .collect();
        if words.first() == Some(&"wisp") {
            let cmd = format!("wisp {}", words[1]);
            need("command", &cmd, docs.contains(&cmd));
        }
        for w in words.iter().filter(|w| w.starts_with('-')) {
            let flag = w.replace("--[no-]", "--");
            let flag = flag.trim_end_matches(['…', '>']);
            need("flag", flag, docs.contains(flag));
        }
    }

    // `const` knobs: the hover table and every one the build reads.
    let mut knobs: Vec<String> = crate::lsp::KNOBS.iter().map(|k| k.0.to_string()).collect();
    let mut build = Vec::new();
    rs_files(&root.join("crates/wisp-build/src"), &mut build);
    for f in &build {
        let text = read_text(f).unwrap_or_default();
        knobs.extend(
            literals(&text, ".constant(\"")
                .into_iter()
                .map(String::from),
        );
    }
    knobs.sort();
    knobs.dedup();
    for k in &knobs {
        need(
            "hover for const",
            k,
            crate::lsp::KNOBS.iter().any(|e| e.0 == k),
        );
        need("const", k, docs.contains(&format!("const {k}")));
    }

    // Hover tables: attributes, directives, blocks.
    for (name, _) in crate::lsp::ATTRS.iter().chain(&crate::lsp::DIRECTIVES) {
        need("attribute", name, docs.contains(name));
    }
    for (name, _, _) in crate::lsp::BLOCKS {
        need("block", name, docs.contains(name));
    }

    // Environment variables.
    let mut all = Vec::new();
    rs_files(&root.join("crates"), &mut all);
    for v in env_reads(&all, &root.join("crates/wisp-cli/src/targets")) {
        match ENV.iter().find(|e| e.0 == v) {
            None => need(
                "unclassified env var (add it to ENV in coverage.rs)",
                &v,
                false,
            ),
            Some((_, true)) => need("env var", &v, mentions(&docs, &v)),
            Some(_) => {}
        }
    }

    // Rust API: the crate root, the prelude, and the methods apps call.
    for n in root_names(&src("crates/wisp/src/lib.rs")) {
        need("wisp::", &n, mentions(&docs, &n));
    }
    let types = [
        ("crates/wisp/src/cx.rs", &["impl Cx"][..]),
        ("crates/wisp/src/lib.rs", &["impl Response"][..]),
        (
            "crates/wisp/src/table.rs",
            &[
                "impl<T> Table<T>",
                "impl<T: Clone> Table<T>",
                "impl<T> Row<T>",
            ][..],
        ),
        ("crates/wisp/src/lib.rs", &["impl<T> Shared<T>"][..]),
    ];
    for (file, heads) in types {
        for m in methods(&src(file), heads) {
            need(
                heads[0].rsplit(' ').next().unwrap_or(""),
                &m,
                mentions(&docs, &m),
            );
        }
    }

    // Browser runtime: `import { .. } from 'wisp'`.
    for file in ["live.js", "element.js"] {
        let text = src(&format!("crates/wisp-shared/src/client/{file}"));
        for line in text.lines() {
            let l = line.strip_prefix("export ").unwrap_or("");
            let l = l.strip_prefix("async ").unwrap_or(l);
            let name = ["function ", "const ", "let ", "class "]
                .iter()
                .find_map(|p| l.strip_prefix(p));
            if let Some(n) = name.map(|n| {
                n.split(|c: char| !ident(c) && c != '$')
                    .next()
                    .unwrap_or("")
            }) {
                // `hydrate` is for wisp.js only.
                if !n.starts_with("__") && n != "hydrate" {
                    need("js export", n, mentions(&docs, n));
                }
            }
        }
    }

    // Macros: `#[name]`, `derive(Name)` and their helper attributes.
    let macros = src("crates/wisp-macros/src/lib.rs");
    for (i, _) in macros.match_indices("#[proc_macro_attribute]") {
        let rest = macros[i..].split("pub fn ").nth(1).unwrap_or("");
        let n = rest.split(|c| !ident(c)).next().unwrap_or("");
        need("attribute macro", n, docs.contains(&format!("#[{n}")));
    }
    for d in macros
        .split("#[proc_macro_derive(")
        .skip(1)
        .map(|r| r.split(']').next().unwrap_or(""))
    {
        let mut parts = d.split(|c: char| !ident(c)).filter(|w| !w.is_empty());
        if let Some(name) = parts.next() {
            need(
                "derive",
                name,
                docs.contains(&format!("derive({name}")) || docs.contains(&format!(", {name}")),
            );
        }
        for a in parts.filter(|w| *w != "attributes") {
            need("helper attribute", a, docs.contains(&format!("#[{a}")));
        }
    }
    let rest_keys = macros.split("const REST_TAKES").nth(1).unwrap_or("");
    let rest_keys = rest_keys.split(";\n").next().unwrap_or("");
    for k in rest_keys
        .split(|c: char| !ident(c))
        .filter(|w| !w.is_empty())
    {
        if !matches!(
            k,
            "str" | "rest" | "takes" | "and" | "ENV_VAR" | "name" | "random"
        ) {
            need(
                "#[rest] key",
                k,
                docs.contains(&format!("{k} =")) || (k == "memory" && mentions(&docs, k)),
            );
        }
    }
    for r in literals(&src("crates/wisp-shared/src/rules.rs"), "name: \"")
        .into_iter()
        .chain(["max_size"])
    {
        need("#[validate] rule", r, mentions(&docs, r));
    }

    // Built-in routes (the dev server's own are not the app's).
    let mut wisp_src = Vec::new();
    rs_files(&root.join("crates/wisp/src"), &mut wisp_src);
    let mut routes: Vec<String> = Vec::new();
    for f in &wisp_src {
        let text = read_text(f).unwrap_or_default();
        routes.extend(
            literals(&text, "\"/_wisp/")
                .into_iter()
                .map(|r| format!("/_wisp/{}", r.trim_end_matches('/'))),
        );
    }
    routes.sort();
    routes.dedup();
    for r in routes
        .iter()
        .filter(|r| r.len() > "/_wisp/".len() && !r.starts_with("/_wisp/dev"))
    {
        need("built-in route", r, docs.contains(r.as_str()));
    }

    // Cargo features of the crates apps and the CLI depend on.
    for toml in ["crates/wisp/Cargo.toml", "crates/wisp-cli/Cargo.toml"] {
        let text = src(toml);
        let feats = text.split("[features]").nth(1).unwrap_or("");
        let feats = feats.split("\n[").next().unwrap_or("");
        for line in feats.lines() {
            if let Some((n, _)) = line.split_once(" = ")
                && !n.starts_with('#')
            {
                need("cargo feature", n, docs.contains(&format!("`{n}`")));
            }
        }
    }

    missing.sort();
    missing.dedup();
    assert!(
        missing.is_empty(),
        "not on the docs site ({}):\n  {}",
        missing.len(),
        missing.join("\n  ")
    );
}
