//! Wisp's compiler. An app's `build.rs` calls [`run`], which scans
//! `src/routes`, compiles every `.wisp` template and writes
//! `$OUT_DIR/wisp.rs` for `wisp::app!()` to include.

mod a11y;
mod codegen;
pub mod csp;
pub mod fmt;
mod fold;
mod i18n;
pub mod ide;
pub mod image;
pub mod inspect;
mod island;
mod js;
mod markdown;
mod model;
pub mod npm;
mod openapi;
mod plugins;
mod pwa;
mod render;
pub mod routes;
pub mod rules;
pub mod rust_scan;
mod shell;
mod sourcemap;
mod stories;
pub mod style;
pub mod template;
mod ty;

// The runtime's HTML context rules (escaping, URL attributes and the
// schemes that run script) and live-page wire protocol: what a build folds
// and writes and what the runtime does are one code.
use wisp_shared::{contexts, protocol};

/// JavaScript without comments and needless whitespace, its names
/// shortened: the browser runtime as release builds serve it (`wisp`'s
/// build.rs).
pub use js::runtime as minify_js;

use std::env;
use std::fs;
use std::path::{Path, PathBuf};

/// Entry point for an app's `build.rs`.
pub fn run() {
    let root = PathBuf::from(
        env::var_os("CARGO_MANIFEST_DIR").expect("wisp_build::run must be called from build.rs"),
    );
    let out_dir = PathBuf::from(
        env::var_os("OUT_DIR").expect("wisp_build::run must be called from build.rs"),
    );
    let release = env::var("PROFILE").is_ok_and(|p| p == "release");
    // `wisp build --sourcemap` asks for source maps in a release build.
    println!("cargo::rerun-if-env-changed=WISP_SOURCEMAP");
    // Browser code's `env.PUBLIC_X`. One it reads that none sets is an
    // error, and a failed build runs this again.
    for (k, _) in public_env(&root) {
        println!("cargo::rerun-if-env-changed={k}");
    }
    let maps = !release || env::var_os("WISP_SOURCEMAP").is_some_and(|v| !v.is_empty());
    // `wisp build`'s prerendered pages, to embed (`const PRERENDER`).
    println!("cargo::rerun-if-env-changed=WISP_PRERENDERED");
    let prerendered = (env::var_os("WISP_PRERENDERED"))
        .filter(|v| release && !v.is_empty())
        .map(PathBuf::from);

    // Only existing paths: Cargo treats a missing one as always changed, which
    // would rebuild the app on every `cargo build`.
    for p in [
        "src",
        "static",
        ".wisp/app.css",
        "package.json",
        ".wisp/npm",
        ".wisp/img",
        ".env",
    ] {
        if root.join(p).exists() {
            println!("cargo::rerun-if-changed={p}");
        }
    }

    // Plugin crates' routes and components, copied in before the scan.
    println!("cargo::rerun-if-changed=Cargo.toml");
    match plugins::sync(&root) {
        Ok(dirs) => dirs
            .iter()
            .for_each(|d| println!("cargo::rerun-if-changed={}", d.display())),
        Err(e) => {
            eprintln!("\nwisp: {e}\n");
            std::process::exit(1);
        }
    }

    match codegen::generate(&codegen::Input {
        root: &root,
        release,
        maps,
        prerendered: prerendered.as_deref(),
    }) {
        Ok(out) => {
            write_if_changed(&out_dir.join("wisp.rs"), &out.code);
            // A dev build serves the scoped CSS from here, which `wisp dev`
            // rewrites when a template changes (see `write_styles`).
            if !release && let Err(e) = save_styles(&root, &out.styles) {
                println!("cargo::warning={e}");
            }
            // The CLI checks first and says them itself.
            if env::var_os("WISP_CLI").is_none() {
                for w in &out.warnings {
                    println!("cargo::warning={w}");
                }
            }
        }
        Err(e) => {
            eprintln!("\nwisp: {e}\n");
            std::process::exit(1);
        }
    }
}

/// A source file's text with `\r\n` (Windows checkouts) as `\n` and no byte
/// order mark, so that a template compiles to the same code either way.
pub fn read_source(path: &Path) -> std::io::Result<String> {
    let text = fs::read_to_string(path)?;
    let text = text.strip_prefix('\u{feff}').unwrap_or(&text);
    Ok(if text.contains('\r') {
        text.replace("\r\n", "\n")
    } else {
        text.to_string()
    })
}

/// Whether the app has a web app manifest: `src/manifest.json`, or a call
/// of `wisp::app_manifest` in `src/hooks.rs`.
pub fn has_manifest(root: &Path) -> bool {
    root.join("src").join("manifest.json").is_file()
        || read_source(&root.join("src").join("hooks.rs"))
            .is_ok_and(|s| s.contains("app_manifest("))
}

/// Whether the app's CSS (`src/app.css`) is Tailwind's input, which the
/// CLI builds into `.wisp/app.css`.
pub fn uses_tailwind(css: &str) -> bool {
    css.contains("@import \"tailwindcss\"") || css.contains("@import 'tailwindcss'")
}

/// The `PUBLIC_*` variables browser code reads as `env.PUBLIC_X`: the
/// process's, and those of `.env` at the app's root the process lacks.
pub fn public_env(root: &Path) -> Vec<(String, String)> {
    let mut vars: Vec<(String, String)> = env::vars_os()
        .filter_map(|(k, v)| Some((k.into_string().ok()?, v.into_string().ok()?)))
        .filter(|(k, _)| k.starts_with("PUBLIC_"))
        .collect();
    let file = read_source(&root.join(".env")).unwrap_or_default();
    for (k, v) in wisp_shared::dotenv::parse(&file).0 {
        if k.starts_with("PUBLIC_") && !vars.iter().any(|(n, _)| *n == k) {
            vars.push((k, v));
        }
    }
    vars.sort();
    vars
}

/// A JSON (and JavaScript) string literal, for files of their own (a
/// module, the OpenAPI document) and values the server writes escaped:
/// not for inside a `<script>`, as `<` stays.
pub(crate) fn json_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            // Older JavaScript took these for line ends, inside strings too.
            '\u{2028}' => out.push_str("\\u2028"),
            '\u{2029}' => out.push_str("\\u2029"),
            c if c < ' ' => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// For `wisp dev`: the static chunks and shape of the template at `rel`
/// (relative to the project root, `/`-separated), and its accessibility
/// warnings (as [`check`] gives them). A running dev build can take new
/// chunks without recompiling as long as the shape is unchanged.
pub fn hot_chunks(root: &Path, rel: &str) -> Result<(Vec<String>, u64, Vec<String>), String> {
    let src = read_source(&root.join(rel)).map_err(|e| format!("{rel}: {e}"))?;
    if rel == "src/app.html" {
        let parts = shell::split(&src).map_err(|e| format!("{rel}: {e}"))?;
        let shape = shell::shape(&parts);
        return Ok((parts.to_vec(), shape, Vec::new()));
    }
    let (rust, markup) = split_front(&src).map_err(|e| format!("{rel}:{e}"))?;
    // A page's forms' fields get their attributes, as in the build: from
    // its block or its `+page.rs`, its route's params and the app's types.
    let rs = rel
        .strip_suffix("+page.wisp")
        .and_then(|dir| read_source(&root.join(dir).join("+page.rs")).ok());
    let items = match (&rust, rs) {
        (Some(block), _) => {
            let marked = rust_scan::mark_default(block);
            rust_scan::scan(&rust_scan::split_items(marked.as_deref().unwrap_or(block)).0)
        }
        (None, Some(rs)) => rust_scan::scan(&rs),
        (None, None) => Ok(rust_scan::Items::default()),
    };
    let segs: Vec<routes::Seg> = (rel.split('/'))
        .filter_map(|d| routes::parse_segment(d).ok().flatten())
        .collect();
    let params: Vec<&str> = segs.iter().filter_map(routes::Seg::param).collect();
    // What does not scan has no fields: the build says what is wrong.
    let fields = items.as_ref().map_or_else(
        |_| Vec::new(),
        |i| rules::fields(i, &params, &shared_types(root)),
    );
    let markup = image::rewrite(&markup, root, false).map_err(|e| format!("{rel}:{e}"))?;
    let drawn = items.as_ref().is_ok_and(rust_scan::Items::drawn) && rel.ends_with("+page.wisp");
    let (t, _) =
        parse_markup(&markup, rust, &fields, rel, drawn).map_err(|e| format!("{rel}:{e}"))?;
    let warnings = (t.lints.iter())
        .map(|l| format!("{rel}:{}: {}", l.line, lint_line(l)))
        .collect();
    Ok((t.chunks, t.shape, warnings))
}

/// A lint as the CLI and the build print it, after its `file:line: `.
pub(crate) fn lint_line(l: &a11y::Lint) -> String {
    format!("{} (a11y-{})", l.msg, l.name)
}

/// A `.wisp` file parsed: its template, and the Rust of its `---` block if
/// it has one, as long as the file (the markup blanked, so every line of
/// Rust is on its line in the file). The block's text is part of the shape:
/// changing it means compiling again. `rel` is its path from the project
/// root, which names its scoped `<style>`'s class. Errors are
/// `line:col: msg`.
pub fn parse_wisp(src: &str, rel: &str) -> Result<(template::Template, Option<String>), String> {
    let (rust, markup) = split_front(src)?;
    parse_markup(&markup, rust, &[], rel, false)
}

/// The markup of a `.wisp` file `split_front` split, parsed: the fields of
/// its action forms given the attributes the browser checks them by
/// ([`rules::fields`]), the Rust of its block in its shape. `drawn`: a page
/// the browser draws (`const SSR: bool = false;`).
pub(crate) fn parse_markup(
    markup: &str,
    rust: Option<String>,
    fields: &[rules::Field],
    rel: &str,
    drawn: bool,
) -> Result<(template::Template, Option<String>), String> {
    let class = style::class(rel);
    let mut t = template::parse_with(markup, fields, &class, drawn).map_err(|e| e.to_string())?;
    if let Some(r) = &rust {
        t.shape ^= fnv1a(r.as_bytes()).rotate_left(1);
    }
    Ok((t, rust))
}

/// The scoped `<style>`s of the templates, `(path, css)`, in path order:
/// the same text from the build and from `wisp dev`.
pub(crate) fn join_styles(mut styles: Vec<(&str, &str)>) -> String {
    styles.sort_unstable();
    let css: Vec<&str> = styles.iter().map(|s| s.1).collect();
    css.join("\n")
}

/// For `wisp dev`, after a template changed: the scoped CSS of every
/// `.wisp` file (and Markdown page) in `src`, written to `protocol::SCOPED_CSS`. Returns whether it
/// changed, and whether there is any; a file that does not parse is left
/// out (the build says what is wrong).
pub fn write_styles(root: &Path) -> Result<(bool, bool), String> {
    let mut files = Vec::new();
    for dir in ["routes", "components"] {
        wisp_files(&root.join("src").join(dir), dir == "routes", &mut files, 0);
    }
    let mut found = Vec::new();
    let mut comps = None;
    for f in &files {
        let rel = f
            .strip_prefix(root)
            .unwrap_or(f)
            .to_string_lossy()
            .replace('\\', "/");
        // A Markdown page is markup once made: its components first.
        let md = |s: String| match rel.ends_with(".md") {
            true => {
                let comps =
                    comps.get_or_insert_with(|| codegen::components(root).unwrap_or_default());
                markdown::page(&s, comps).map(|m| m.wisp)
            }
            false => Ok(s),
        };
        if let Ok((t, _)) = read_source(f)
            .map_err(|e| e.to_string())
            .and_then(md)
            .and_then(|s| parse_wisp(&s, &rel))
        {
            found.extend(t.style.map(|s| (rel, s)));
        }
    }
    let css = join_styles(
        found
            .iter()
            .map(|(r, s)| (r.as_str(), s.as_str()))
            .collect(),
    );
    let old = fs::read_to_string(root.join(protocol::SCOPED_CSS)).unwrap_or_default();
    if old != css {
        save_styles(root, &css)?;
    }
    Ok((old != css, !css.is_empty()))
}

fn wisp_files(dir: &Path, routes: bool, out: &mut Vec<PathBuf>, depth: usize) {
    for e in fs::read_dir(dir).into_iter().flatten().flatten() {
        let p = e.path();
        if p.is_dir() && depth < 32 {
            wisp_files(&p, routes, out, depth + 1);
        } else {
            let name = e.file_name().to_string_lossy().into_owned();
            let page = name.starts_with('+') || name.ends_with(".md");
            if (name.ends_with(".wisp") || (routes && name.ends_with(".md")))
                && !routes::editor_temp(&name)
                && (!routes || page)
            {
                out.push(p);
            }
        }
    }
}

/// Writes `protocol::SCOPED_CSS` when it would change (an empty one only over one
/// there).
fn save_styles(root: &Path, css: &str) -> Result<(), String> {
    let path = root.join(protocol::SCOPED_CSS);
    if css.is_empty() && !path.exists() {
        return Ok(());
    }
    if fs::read(&path).is_ok_and(|old| old == css.as_bytes()) {
        return Ok(());
    }
    fs::create_dir_all(root.join(".wisp"))
        .and_then(|()| fs::write(&path, css))
        .map_err(|e| format!("{}: {e}", path.display()))
}

/// The types the app's own modules (`src/*.rs`) define, in name order (so
/// that the first of two of one name is the same on every machine); a
/// file that does not scan is skipped. Endpoints and forms name them.
pub(crate) fn shared_types(root: &Path) -> Vec<rust_scan::TypeItem> {
    let mut files: Vec<PathBuf> = (fs::read_dir(root.join("src")).into_iter().flatten())
        .flatten()
        .map(|e| e.path())
        .filter(|f| f.extension().is_some_and(|e| e == "rs"))
        .collect();
    files.sort();
    (files.iter())
        .filter_map(|f| rust_scan::scan(&read_source(f).ok()?).ok())
        .flat_map(|items| items.types)
        .collect()
}

/// The `.live()` tables of `src/*.rs`: (static, channel), see
/// `Items::live_tables`.
pub(crate) fn live_tables(root: &Path) -> Vec<(String, String)> {
    let mut files: Vec<PathBuf> = (fs::read_dir(root.join("src")).into_iter().flatten())
        .flatten()
        .map(|e| e.path())
        .filter(|f| f.extension().is_some_and(|e| e == "rs"))
        .collect();
    files.sort();
    (files.iter())
        .filter_map(|f| {
            let src = read_source(f).ok()?;
            rust_scan::scan(&rust_scan::name_saved(&src).unwrap_or(src)).ok()
        })
        .flat_map(|items| items.live_tables())
        .collect()
}

/// Splits off the `---` block of Rust a page or layout may start with:
///
/// ```text
/// ---
/// let post = posts::find(&slug).or_404()?;
/// ---
/// <h1>{post.title}</h1>
/// ```
///
/// Both halves keep the file's lines: the Rust with the markup blanked, the
/// markup with the block's lines left empty.
pub(crate) fn split_front(src: &str) -> Result<(Option<String>, String), String> {
    let lines: Vec<&str> = src.split('\n').collect();
    let Some(open) = lines.iter().position(|l| !l.trim().is_empty()) else {
        return Ok((None, src.to_string()));
    };
    if lines[open].trim() != "---" {
        return Ok((None, src.to_string()));
    }
    let Some(close) = lines[open + 1..].iter().position(|l| l.trim() == "---") else {
        return Err(format!(
            "{}:1: this `---` starts a block of Rust, which needs a `---` line after it",
            open + 1
        ));
    };
    let close = open + 1 + close;
    let pick = |rust: bool| -> String {
        let kept: Vec<&str> = lines
            .iter()
            .enumerate()
            .map(|(k, l)| {
                let inside = k > open && k < close;
                let markup = k > close;
                if (rust && inside) || (!rust && markup) {
                    *l
                } else {
                    ""
                }
            })
            .collect();
        kept.join("\n")
    };
    Ok((Some(pick(true)), pick(false)))
}

/// Checks the whole project the way `run` does, without writing anything.
/// Returns the esm.sh paths of the npm modules the app's browser code
/// imports, which `wisp build` downloads into `.wisp/npm` for a release,
/// and the templates' accessibility warnings (`file:line: what`).
pub fn check(root: &Path) -> Result<(Vec<String>, Vec<String>), String> {
    codegen::check(&codegen::Input {
        root,
        release: false,
        maps: false,
        prerendered: None,
    })
}

pub use codegen::{Hot, HotTemplate};

/// For `wisp dev`: the app as its dev build compiles it, as far as a
/// running dev build can take it without a compile, and its accessibility
/// warnings. Errors as [`check`]'s.
pub fn hot(root: &Path) -> Result<Hot, String> {
    codegen::hot(&codegen::Input {
        root,
        release: false,
        maps: true,
        prerendered: None,
    })
}

/// For `wisp check --types`: the files `.wisp/types` holds for `tsc`, by
/// path there (see the CLI's `types.rs`), with `probed` (the app's own
/// print of its block values' types, or `""`), and whether a script reads
/// a block's values, which only that types.
pub fn types(root: &Path, probed: &str) -> Result<(Vec<(String, String)>, bool), String> {
    codegen::types(
        &codegen::Input {
            root,
            release: false,
            maps: false,
            prerendered: None,
        },
        probed,
    )
}

/// The TypeScript client of the project's `+server.rs` endpoints: a module
/// whose `client({ base, token })` has a typed method per operation, for
/// `wisp build --client ts`. Empty when the app has no endpoints.
pub fn client_ts(root: &Path) -> Result<String, String> {
    codegen::generate_all(&codegen::Input {
        root,
        release: false,
        maps: false,
        prerendered: None,
    })
    .map(|(_, ts)| ts)
}

/// `base = "/app"` of `[package.metadata.wisp]`, for `wisp build` to build
/// with as `WISP_BASE`.
pub fn app_base(root: &std::path::Path) -> Option<String> {
    plugins::base(root)
}

/// The base path the app is served under (`WISP_BASE`), empty for none.
pub const BASE: &str = protocol::BASE;

/// The `?v=` of the browser runtime (`/_app/wisp.js`, `/_app/live.js`):
/// the version and a hash of both files, so a changed runtime has a new
/// address and no browser keeps an old one from its cache.
pub fn runtime_version() -> &'static str {
    static V: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    V.get_or_init(|| {
        let js = [wisp_shared::WISP_JS, wisp_shared::LIVE_JS].concat();
        format!(
            "{}-{:08x}",
            env!("CARGO_PKG_VERSION"),
            fnv1a(js.as_bytes()) as u32
        )
    })
}

/// FNV-1a, 64-bit. Used for shape and asset hashes, not for security.
pub const fn fnv1a(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let mut i = 0;
    while i < bytes.len() {
        h ^= bytes[i] as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
        i += 1;
    }
    h
}

/// Rewriting identical output would bump its mtime and trigger rebuilds.
fn write_if_changed(path: &Path, contents: &str) {
    if fs::read(path).is_ok_and(|old| old == contents.as_bytes()) {
        return;
    }
    fs::write(path, contents).unwrap_or_else(|e| panic!("writing {}: {e}", path.display()));
}

#[cfg(test)]
mod tests {
    #[test]
    fn fnv_vectors() {
        assert_eq!(super::fnv1a(b""), 0xcbf29ce484222325);
        assert_eq!(super::fnv1a(b"a"), 0xaf63dc4c8601ec8c);
        assert_eq!(super::fnv1a(b"foobar"), 0x85944171f73967e8);
    }
}
