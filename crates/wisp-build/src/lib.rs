//! Wisp's compiler. An app's `build.rs` calls [`run`], which scans
//! `src/routes`, compiles every `.wisp` template and writes
//! `$OUT_DIR/wisp.rs` for `wisp::app!()` to include.

mod a11y;
pub mod auto;
mod client_lets;
mod codegen;
mod config;
pub mod csp;
pub mod fmt;
mod fold;
mod fonts;
mod i18n;
pub mod ide;
pub mod image;
pub mod inspect;
mod island;
mod js;
mod loading;
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
        ".wisp/fonts",
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

/// For `wisp dev`, which compiles the app without cargo: writes `code` (a
/// [`hot`]'s `code`) where the build script would, `out_dir/wisp.rs`.
/// Unchanged text is left alone; an error says why it could not be written.
pub fn write_generated(out_dir: &Path, code: &str) -> Result<(), String> {
    let path = out_dir.join("wisp.rs");
    if fs::read(&path).is_ok_and(|old| old == code.as_bytes()) {
        return Ok(());
    }
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, code)
        .and_then(|()| fs::rename(&tmp, &path))
        .map_err(|e| {
            let _ = fs::remove_file(&tmp);
            format!("{}: {e}", path.display())
        })
}

/// Whether the app has plugins or layers: their routes are copied into
/// `src` by the build script, which a build without it would miss.
pub fn has_plugins(root: &Path) -> bool {
    let toml = fs::read_to_string(root.join("Cargo.toml")).unwrap_or_default();
    !plugins::used(&toml).is_empty() || !plugins::list(&toml, "extends").is_empty()
}

/// A source file's text with `\r\n` (Windows checkouts) as `\n` and no byte
/// order mark, so that a template compiles to the same code either way.
pub fn read_source(path: &Path) -> std::io::Result<String> {
    let text = fs::read_to_string(path).map_err(|e| match e.kind() {
        std::io::ErrorKind::InvalidData => std::io::Error::new(
            e.kind(),
            "not UTF-8 text: save the file as UTF-8 in your editor",
        ),
        _ => e,
    })?;
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
            let marked = rust_scan::mark_actions(block, &markup);
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
pub(crate) fn join_styles(root: &Path, mut styles: Vec<(&str, &str)>) -> String {
    styles.sort_unstable();
    let mut css = vec![plugins::layer_css(root), fonts::css(root)];
    css.retain(|f| !f.is_empty());
    css.extend(styles.iter().map(|s| s.1.to_string()));
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
                markdown::page(&s, comps, false).map(|m| m.wisp)
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
        root,
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

/// The `mod server { … }` of the `---` block of the page `wisp`: the
/// route's endpoints, as a `+server.rs` beside it would hold them (as long
/// as the file, the rest blanked), and the line the module starts on.
/// `None` when the page has no such module or does not read.
pub(crate) fn block_server(wisp: &Path) -> Option<(String, usize)> {
    let (rust, _) = split_front(&read_source(wisp).ok()?).ok()?;
    let (_, server, line) = rust_scan::split_server(&rust_scan::split_items(&rust?).0)?;
    Some((server, line))
}

/// The Rust of route `r`, each part with the file it is in: its page's (a
/// `+page.rs`, or its `+page.wisp`'s `---` block but for `mod server`) and
/// its endpoints' (a `+server.rs`, or that `mod server`'s contents). A
/// block's keeps the file's lines, the rest blanked. Errs on a Rust file
/// that does not read.
pub fn route_rust(r: &routes::Route) -> Result<[Option<(PathBuf, String)>; 2], String> {
    let read = |p: PathBuf| match read_source(&p) {
        Ok(s) => Ok((p, s)),
        Err(e) => Err(format!("{}: {e}", p.display())),
    };
    let wisp = r.dir.join(&r.page_file);
    let block = (r.page && r.md.is_none() && !r.page_rs)
        .then(|| split_front(&read_source(&wisp).ok()?).ok()?.0)
        .flatten();
    let page = match (r.page_rs, block) {
        (true, _) => Some(read(r.dir.join("+page.rs"))?),
        (false, Some(b)) => match rust_scan::split_server(&b) {
            Some((rest, _, _)) => Some((wisp.clone(), rest)),
            None => Some((wisp.clone(), b)),
        },
        (false, None) => None,
    };
    let rs = r.dir.join("+server.rs");
    let server = match r.server {
        false => None,
        true if rs.is_file() => Some(read(rs)?),
        true => block_server(&wisp).map(|(s, _)| (wisp, s)),
    };
    Ok([page, server])
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
/// markup with the block's lines left empty. A `let` of a literal that only
/// the browser reads moves into the client script (see `client_lets`).
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
    let (mut rust, mut markup) = (pick(true), pick(false));
    // A block that was only browser state is gone, as if never written.
    let moved = client_lets::fold(&mut rust, &mut markup, open, close);
    Ok(((!moved || !rust.trim().is_empty()).then_some(rust), markup))
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

pub use codegen::{Hot, HotTemplate, Weight};

/// For the editor: the names files may use with no `use` line (see
/// [`auto`]), as far as they can be read; none when they cannot.
pub fn auto_names(root: &Path) -> auto::Auto {
    auto::Auto::load(root, &codegen::mod_files(root)).unwrap_or_default()
}

/// For `wisp check --explain-imports`: per file (from the root), the names
/// it uses with no `use` line, each with the path it is imported from.
/// Errors as [`check`]'s.
pub fn imports(root: &Path) -> Result<crate::auto::FileImports, String> {
    codegen::imports(&codegen::Input {
        root,
        release: false,
        maps: false,
        prerendered: None,
    })
}

/// For `wisp build --analyze`: each route's browser files as a release
/// build serves them.
pub fn analyze(root: &Path) -> Result<Vec<(String, Vec<Weight>)>, String> {
    codegen::analyze(&codegen::Input {
        root,
        release: true,
        maps: false,
        prerendered: None,
    })
}

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

/// The OpenAPI 3.1 document of the project's endpoints, pages and form
/// actions: the one `/_wisp/openapi.json` serves, for `wisp openapi`. Empty
/// when the app has no endpoints and no actions.
pub fn openapi(root: &Path) -> Result<String, String> {
    codegen::openapi(&codegen::Input {
        root,
        release: false,
        maps: false,
        prerendered: None,
    })
}

/// [`openapi`]'s document indented, a member a line: the file `wisp
/// openapi` writes and `--check` compares.
pub fn pretty_json(json: &str) -> String {
    openapi::pretty(json)
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
    V.get_or_init(wisp_shared::runtime_version)
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
    // A temp file renamed over the target: a killed build never leaves a
    // half-written file for rustc to choke on.
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, contents)
        .and_then(|()| fs::rename(&tmp, path))
        .unwrap_or_else(|e| {
            let _ = fs::remove_file(&tmp);
            panic!(
                "wisp: cannot write {}: {e}; make the target directory writable (or run `cargo clean`) and build again",
                path.display()
            )
        });
}

#[cfg(test)]
mod tests {
    /// Malformed templates, Rust and JS: every input returns (Ok or Err),
    /// none panics or overflows the stack. A tiny xorshift keeps it
    /// deterministic.
    #[test]
    fn fuzzed_sources_never_panic() {
        const BITS: &[&str] = &[
            "<div>",
            "</div>",
            "<p class=\"{a}\">",
            "{#if a}",
            "{:else}",
            "{/if}",
            "{#each xs as x}",
            "{/each}",
            "{#await f}",
            "{/await}",
            "{a}",
            "{",
            "}",
            "---\n",
            "<script>",
            "</script>",
            "<style>",
            "\"",
            "'",
            "\\",
            "*/",
            "//",
            "\r",
            "\n",
            "\u{202e}",
            "\u{2028}",
            "日本",
            "<!--",
            "-->",
            "{@html x}",
            "{@render c()}",
            "{#snippet s()}",
            "{/snippet}",
            "<",
            ">",
            "=",
            "`",
            "${",
        ];
        let mut x = 0x2545_F491_4F6C_DD1Du64;
        let mut next = || {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x
        };
        for _ in 0..4000 {
            let n = (next() % 24) as usize;
            let src: String = (0..n)
                .map(|_| BITS[(next() % BITS.len() as u64) as usize])
                .collect();
            for cut in [src.len(), src.len() / 2] {
                let s = &src[..src.floor_char_boundary(cut)];
                let _ = super::parse_wisp(s, "f.wisp");
                let _ = super::fmt::format(s, "2024");
                let _ = super::rust_scan::scan(s);
                let _ = super::js::tokens(s);
            }
        }
    }

    /// 10k levels of nesting: an Err or Ok, never a stack overflow.
    #[test]
    fn deep_nesting_does_not_overflow() {
        let n = 10_000;
        // The last two make one huge line.
        for (open, close) in [
            ("<div>", "</div>"),
            ("{#if a}", "{/if}"),
            ("{#each a as b}", "{/each}"),
            ("<i>", ""),
            ("{", "}"),
            ("(", ")"),
            ("x", ""),
            ("{a}", ""),
        ] {
            let src = open.repeat(n) + &close.repeat(n);
            let _ = super::parse_wisp(&src, "f.wisp");
            let _ = super::fmt::format(&src, "2024");
            let _ = super::rust_scan::scan(&src);
            let _ = super::js::tokens(&src);
        }
    }

    #[test]
    fn fnv_vectors() {
        assert_eq!(super::fnv1a(b""), 0xcbf29ce484222325);
        assert_eq!(super::fnv1a(b"a"), 0xaf63dc4c8601ec8c);
        assert_eq!(super::fnv1a(b"foobar"), 0x85944171f73967e8);
    }

    /// The minified runtime `wisp` serves in release builds is kept in the
    /// repo (`wisp_shared::WISP_MIN_JS`). A stale copy is rewritten here, and
    /// the test fails until the next run builds with the new one.
    #[test]
    fn minified_runtime_is_current() {
        let dir =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../wisp-shared/src/client");
        let mut stale = Vec::new();
        for (name, src, kept) in [
            (
                "wisp.min.js",
                wisp_shared::WISP_JS,
                wisp_shared::WISP_MIN_JS,
            ),
            (
                "live.min.js",
                wisp_shared::LIVE_JS,
                wisp_shared::LIVE_MIN_JS,
            ),
        ] {
            let min = super::minify_js(src);
            if min != kept {
                std::fs::write(dir.join(name), &min).unwrap();
                stale.push(name);
            }
        }
        assert!(stale.is_empty(), "rewrote {stale:?}: run the tests again");
    }
}
