//! `wisp dev`'s rebuild without cargo.
//!
//! An edit to the app's own Rust or templates needs one thing: the app
//! crate compiled again. Cargo adds to that its own start (resolving, the
//! fingerprints of every dependency) and the build script's run, which is
//! this program's `wisp_build::hot` over again: about 0.15 s on a small app
//! and 0.3 s on a large one, of a rebuild that takes 0.9 s to 1.4 s.
//!
//! So the first build, and any build after a change that is not a `.rs` or
//! `.wisp` file under `src`, goes through cargo, which records how it ran
//! rustc for the app (this program is cargo's `RUSTC_WRAPPER` for that run:
//! [`wrap`]). Later edits write the generated code where the build script
//! would and run that rustc command again: same arguments, same incremental
//! cache, no cargo. Whatever is unusual (a plugin, a library beside the
//! binary, a failure that is not the app's own) is cargo's, as before.
//!
//! On Windows the replay also links with `rust-lld`, which shipped with
//! rustc: about 0.1 s less than `link.exe`. A link that fails is tried
//! again the old way.

use crate::cargo::{self, Build};
use crate::term;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use wisp_shared::json;

/// Where the wrapper writes the recipe, set only for the cargo run whose
/// rustc call is to be recorded.
const RECORD: &str = "WISP_RUSTC_RECORD";
/// The binary's name (the package's), so the wrapper records that rustc
/// call and no other.
const BIN: &str = "WISP_RUSTC_BIN";

/// How cargo ran rustc for the app's binary.
#[derive(Debug, PartialEq)]
pub struct Recipe {
    rustc: String,
    cwd: String,
    args: Vec<String>,
    env: Vec<(String, String)>,
}

/// The recipe's file, in the app's `.wisp/run`.
pub fn recipe_path(root: &Path) -> PathBuf {
    root.join(".wisp").join("run").join("rustc")
}

/// What the command line of `wisp` is when cargo runs it as `RUSTC_WRAPPER`
/// (`wisp <rustc> <arguments>`): records the call when it is the app's
/// binary, then runs rustc. `None` when this is not that, so `main` goes on.
pub fn wrap() -> Option<ExitCode> {
    let record = std::env::var_os(RECORD)?;
    let mut args = std::env::args_os().skip(1);
    // Whatever cargo names as rustc (`RUSTC=` may be any program) runs:
    // under the record variable this is never the CLI's own command line.
    let rustc = args.next()?;
    let args: Vec<OsString> = args.collect();
    let named_rustc = Path::new(&rustc).file_stem().is_some_and(|s| s == "rustc");
    if let Some(r) = Recipe::of_call(&rustc, &args).filter(|_| named_rustc) {
        let _ = r.save(Path::new(&record));
    }
    let code = Command::new(&rustc)
        .args(&args)
        .status()
        .ok()
        .and_then(|s| s.code());
    Some(ExitCode::from(exit_byte(code)))
}

/// rustc's exit status as one byte: a failure is never 0 (a Windows status
/// such as 0xC0000100 truncates to it), and no status is 1.
fn exit_byte(code: Option<i32>) -> u8 {
    match code {
        Some(0) => 0,
        Some(c) if c & 0xff != 0 => c as u8,
        _ => 1,
    }
}

/// The variables cargo sets for rustc that the call needs again: the
/// package's (`env!("CARGO_PKG_NAME")`), the build script's output folder,
/// and where to find the libraries proc macros load. Not the rest of the
/// environment: it holds secrets, and is the CLI's own by then.
fn kept(name: &str) -> bool {
    name.starts_with("CARGO_PKG_")
        || matches!(
            name,
            "CARGO"
                | "CARGO_MANIFEST_DIR"
                | "CARGO_MANIFEST_PATH"
                | "CARGO_BIN_NAME"
                | "CARGO_CRATE_NAME"
                | "CARGO_PRIMARY_PACKAGE"
                | "OUT_DIR"
                | "LD_LIBRARY_PATH"
                | "DYLD_LIBRARY_PATH"
                | "DYLD_FALLBACK_LIBRARY_PATH"
        )
        || (cfg!(windows) && name.eq_ignore_ascii_case("PATH"))
}

impl Recipe {
    /// The call, if it compiles the app's own binary: not a dependency, a
    /// test or a check.
    fn of_call(rustc: &OsString, args: &[OsString]) -> Option<Recipe> {
        let var = |k: &str| std::env::var(k).ok();
        var("CARGO_PRIMARY_PACKAGE")?;
        let bin = var("CARGO_BIN_NAME")?;
        if var(BIN)? != bin || var("CARGO_CRATE_NAME")? != bin.replace('-', "_") {
            return None;
        }
        let args: Vec<String> = args
            .iter()
            .map(|a| a.to_str().map(String::from))
            .collect::<Option<_>>()?;
        let pair = |a: &str, b: &str| args.windows(2).any(|w| w[0] == a && w[1] == b);
        let emits_link = args.iter().any(|a| {
            a.strip_prefix("--emit=")
                .is_some_and(|e| e.split(',').any(|e| e == "link"))
        });
        if !pair("--crate-type", "bin") || !emits_link || args.iter().any(|a| a == "--test") {
            return None;
        }
        Some(Recipe {
            rustc: rustc.to_str()?.to_string(),
            cwd: std::env::current_dir().ok()?.to_str()?.to_string(),
            args,
            env: std::env::vars().filter(|(k, _)| kept(k)).collect(),
        })
    }

    /// One line each, `\` and line breaks escaped: `R` rustc, `D` its
    /// working folder, `A` an argument, `E` a variable.
    fn text(&self) -> String {
        let mut out = String::new();
        let mut line = |tag: char, s: &str| {
            out.push(tag);
            out.push(' ');
            out.push_str(
                &s.replace('\\', "\\\\")
                    .replace('\n', "\\n")
                    .replace('\r', "\\r"),
            );
            out.push('\n');
        };
        line('R', &self.rustc);
        line('D', &self.cwd);
        for a in &self.args {
            line('A', a);
        }
        for (k, v) in &self.env {
            line('E', &format!("{k}={v}"));
        }
        out
    }

    fn parse(text: &str) -> Option<Recipe> {
        let mut r = Recipe {
            rustc: String::new(),
            cwd: String::new(),
            args: Vec::new(),
            env: Vec::new(),
        };
        for line in text.lines() {
            let (tag, rest) = line.split_once(' ')?;
            let mut value = String::with_capacity(rest.len());
            let mut chars = rest.chars();
            while let Some(c) = chars.next() {
                match (c, c == '\\') {
                    (_, true) => value.push(match chars.next()? {
                        'n' => '\n',
                        'r' => '\r',
                        c => c,
                    }),
                    _ => value.push(c),
                }
            }
            match tag {
                "R" => r.rustc = value,
                "D" => r.cwd = value,
                "A" => r.args.push(value),
                "E" => r
                    .env
                    .push(value.split_once('=').map(|(k, v)| (k.into(), v.into()))?),
                _ => return None,
            }
        }
        (!r.rustc.is_empty() && !r.args.is_empty()).then_some(r)
    }

    fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, self.text())?;
        std::fs::rename(&tmp, path)
    }

    /// The recipe the last cargo build recorded, if it did.
    pub fn load(root: &Path) -> Option<Recipe> {
        Recipe::parse(&std::fs::read_to_string(recipe_path(root)).ok()?)
    }

    fn var(&self, name: &str) -> Option<&str> {
        let (_, v) = self.env.iter().find(|(k, _)| k == name)?;
        Some(v.as_str())
    }

    /// Where the build script's output goes.
    fn out_dir(&self) -> Option<&str> {
        self.var("OUT_DIR")
    }

    /// Whether the call already names a linker, which is then left alone.
    fn names_linker(&self) -> bool {
        self.args.iter().any(|a| a.starts_with("linker="))
    }
}

/// A cargo build of the app, recording how it runs rustc to
/// [`recipe_path`], unless the person has a rustc wrapper of their own
/// (sccache), which it would replace.
pub fn record(root: &Path, quiet: bool) -> Build {
    let path = recipe_path(root);
    let own = ["RUSTC_WRAPPER", "RUSTC_WORKSPACE_WRAPPER"]
        .iter()
        .any(|k| std::env::var_os(k).is_some_and(|v| !v.is_empty()))
        || wrapper_configured(root);
    let exe = std::env::current_exe().ok();
    let (exe, path, name) = (
        exe.as_deref().and_then(Path::to_str),
        path.to_str(),
        cargo::package_name(root),
    );
    let b = match (own, exe, path, name.as_deref()) {
        (false, Some(exe), Some(path), Some(name)) => cargo::build_for(
            root,
            false,
            quiet,
            &[],
            &[("RUSTC_WRAPPER", exe), (RECORD, path), (BIN, name)],
        ),
        // Not recorded: no recipe, so no build without cargo.
        _ => {
            let _ = std::fs::remove_file(recipe_path(root));
            cargo::build(root, false, quiet)
        }
    };
    // A binary cargo found fresh keeps the recipe it was built by; one that
    // did not build may have been cut off before its dependencies were.
    if !b.ok {
        let _ = std::fs::remove_file(recipe_path(root));
    }
    b
}

/// Whether a cargo configuration names a rustc wrapper (`rustc-wrapper =
/// "sccache"`): the app's folder and those above it, and cargo's home.
fn wrapper_configured(root: &Path) -> bool {
    let home = std::env::var_os("CARGO_HOME")
        .map(PathBuf::from)
        .or_else(|| {
            let user = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME"))?;
            Some(PathBuf::from(user).join(".cargo"))
        });
    let abs = std::path::absolute(root).unwrap_or_else(|_| root.to_path_buf());
    let dirs = abs.ancestors().map(|d| d.join(".cargo")).chain(home);
    dirs.flat_map(|d| [d.join("config.toml"), d.join("config")])
        .filter_map(|f| std::fs::read_to_string(f).ok())
        .any(|text| text.contains("rustc-wrapper") || text.contains("rustc_wrapper"))
}

/// What a rebuild without cargo came to.
pub enum Quick {
    Done(Build),
    /// It could not say: cargo is to build.
    Cargo,
}

/// Whether edits to the app can be built without cargo at all: a library
/// beside the binary would be stale, plugins' routes are copied in by the
/// build script, and a build script of the app's own may do more than
/// Wisp's.
pub fn possible(root: &Path) -> bool {
    let standard = std::fs::read_to_string(root.join("build.rs"))
        .is_ok_and(|s| hash(&s, false) == hash("fn main() { wisp_build::run(); }", false));
    standard && !root.join("src").join("lib.rs").exists() && !wisp_build::has_plugins(root)
}

/// Builds the app from `code` (what the build script would write) the way
/// the recipe says. `lld` is whether to link with `rust-lld`; it turns
/// itself off when a link with it fails.
pub fn build(root: &Path, recipe: &Recipe, code: &str, lld: &mut bool) -> Quick {
    let Some(out) = recipe.out_dir() else {
        return Quick::Cargo;
    };
    if let Err(e) = wisp_build::write_generated(Path::new(out), code) {
        term::warn(&e);
        return Quick::Cargo;
    }
    let linker = *lld && cfg!(all(windows, target_env = "msvc")) && !recipe.names_linker();
    let mut shown = Vec::new();
    let Some(mut b) = run(root, recipe, linker, &mut shown) else {
        return Quick::Cargo;
    };
    // The linker is not the app's fault: the old one, then.
    if linker && !b.ok && b.errors.contains("linking with") {
        *lld = false;
        shown.clear();
        match run(root, recipe, false, &mut shown) {
            Some(again) => b = again,
            None => return Quick::Cargo,
        }
    }
    // A failure none of the app's files is named in is for cargo to tell:
    // a dependency it changed, a file rustc could not read.
    if !b.ok && b.first.is_none() {
        return Quick::Cargo;
    }
    for text in shown {
        eprint!("{text}");
    }
    Quick::Done(b)
}

/// rustc as the recipe has it; `shown` gets the diagnostics' text. `None`
/// when it did not run.
fn run(root: &Path, recipe: &Recipe, lld: bool, shown: &mut Vec<String>) -> Option<Build> {
    let mut cmd = Command::new(&recipe.rustc);
    cmd.args(&recipe.args);
    if lld {
        cmd.args(["-C", "linker=rust-lld", "-C", "linker-flavor=lld-link"]);
    }
    cmd.envs(recipe.env.iter().cloned());
    cmd.current_dir(&recipe.cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    let mut child = cmd.spawn().ok()?;
    let mut b = Build {
        ok: false,
        exe: None,
        errors: String::new(),
        count: 0,
        first: None,
    };
    term::each_line(child.stderr.take()?, |line| {
        let Ok(msg) = json::parse(line) else {
            return;
        };
        if let Some(path) = msg
            .str("artifact")
            .filter(|_| msg.str("emit") == Some("link"))
        {
            let path = PathBuf::from(path);
            let ext = path.extension().and_then(|e| e.to_str());
            if ext.unwrap_or("") == std::env::consts::EXE_EXTENSION {
                b.exe = Some(path);
            }
        } else if !summary(msg.str("message").unwrap_or_default())
            && let Some(text) = cargo::diagnostic(&mut b, &msg, root)
        {
            shown.push(text);
        }
    });
    b.ok = child.wait().ok()?.success();
    if b.ok && b.exe.is_none() {
        return None;
    }
    if !b.ok && b.errors.is_empty() {
        b.errors
            .push_str("rustc failed. The terminal has the details.");
    }
    Some(b)
}

/// rustc's closing lines ("aborting due to 1 previous error", "2 warnings
/// emitted"), which cargo does not pass on and counts for itself.
fn summary(message: &str) -> bool {
    message.starts_with("aborting due to")
        || message
            .strip_suffix(" emitted")
            .and_then(|m| m.split_once(' '))
            .is_some_and(|(n, w)| n.bytes().all(|b| b.is_ascii_digit()) && w.starts_with("warning"))
}

// ---- edits that change no code ---------------------------------------------------

/// A hash of Rust source's tokens and where each starts: the same for a
/// file whose comments or spacing changed with every token in place, and
/// different for anything the compiler would read differently (doc comments
/// are attributes and count; so does every character of a literal, and each
/// token's line and column, which `line!()` and panic locations show).
pub fn code_hash(src: &str) -> u64 {
    hash(src, true)
}

/// [`code_hash`], with each token's place (`at`) or without.
fn hash(src: &str, at: bool) -> u64 {
    let b = src.as_bytes();
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    let mut eat = |bytes: &[u8]| {
        for &c in bytes {
            h = (h ^ c as u64).wrapping_mul(0x0000_0100_0000_01b3);
        }
    };
    let class = |c: u8| match c {
        b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'_' | 0x80.. => 1,
        b'!' | b'#' | b'$' | b'%' | b'&' | b'*' | b'+' | b'-' | b'.' | b'/' | b':' | b'<'
        | b'=' | b'>' | b'?' | b'@' | b'^' | b'|' | b'~' => 2,
        _ => 0,
    };
    // The last byte taken, and whether space came after it.
    let (mut last, mut gap) = (0u8, false);
    // The line and the byte its line starts at, counted up to `scanned`.
    let (mut line, mut line_start, mut scanned) = (0u64, 0usize, 0usize);
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        let rest = &b[i..];
        let end = if c.is_ascii_whitespace() {
            gap = true;
            i += 1;
            continue;
        } else if rest.starts_with(b"//") {
            let line = rest.iter().position(|&x| x == b'\n').unwrap_or(rest.len());
            let doc = (rest.starts_with(b"///") && !rest.starts_with(b"////"))
                || rest.starts_with(b"//!");
            if !doc {
                gap = true;
                i += line;
                continue;
            }
            line
        } else if rest.starts_with(b"/*") {
            let mut depth = 0;
            let mut n = 0;
            while n < rest.len() {
                if rest[n..].starts_with(b"/*") {
                    depth += 1;
                    n += 2;
                } else if rest[n..].starts_with(b"*/") {
                    depth -= 1;
                    n += 2;
                    if depth == 0 {
                        break;
                    }
                } else {
                    n += 1;
                }
            }
            let doc = (rest.starts_with(b"/**") && !rest.starts_with(b"/***") && n > 4)
                || rest.starts_with(b"/*!");
            if !doc {
                gap = true;
                i += n;
                continue;
            }
            n
        } else {
            literal(rest).unwrap_or(1)
        };
        let first = class(c);
        for (n, &x) in b.iter().enumerate().take(i).skip(scanned) {
            if x == b'\n' {
                line += 1;
                line_start = n + 1;
            }
        }
        scanned = i;
        if at {
            eat(&line.to_le_bytes());
            eat(&((i - line_start) as u64).to_le_bytes());
        }
        if gap && last != 0 && first != 0 && class(last) == first {
            eat(b" ");
        }
        gap = false;
        eat(&rest[..end]);
        last = rest[end - 1];
        i += end;
    }
    h
}

/// The length of the string, byte string, raw string or character literal
/// at the start of `s`; `None` for anything else, a lifetime too.
fn literal(s: &[u8]) -> Option<usize> {
    let mut i = 0;
    let raw = |s: &[u8]| -> Option<usize> {
        // `r#"..."#` from the `r`.
        let hashes = s[1..].iter().take_while(|&&c| c == b'#').count();
        (s.get(1 + hashes) == Some(&b'"')).then_some(())?;
        let close: Vec<u8> = std::iter::once(b'"')
            .chain(std::iter::repeat_n(b'#', hashes))
            .collect();
        let body = 2 + hashes;
        let at = s[body..]
            .windows(close.len())
            .position(|w| w == &close[..])?;
        Some(body + at + close.len())
    };
    match s[0] {
        b'r' => return raw(s),
        b'b' if s.get(1) == Some(&b'r') => return raw(&s[1..]).map(|n| n + 1),
        b'b' if matches!(s.get(1), Some(b'"' | b'\'')) => i = 1,
        b'c' if s.get(1) == Some(&b'"') => i = 1,
        b'"' | b'\'' => {}
        _ => return None,
    }
    let quote = s[i];
    if quote == b'\'' {
        // A character, or a lifetime (`'a`, no closing quote).
        let n = match s.get(i + 1)? {
            // An escape, then up to the closing quote.
            b'\\' => {
                let close = s.get(i + 3..)?.iter().position(|&c| c == b'\'')?;
                i + 3 + close + 1
            }
            &lead => {
                let len = match lead {
                    0..=0x7f => 1,
                    0xc0..=0xdf => 2,
                    0xe0..=0xef => 3,
                    _ => 4,
                };
                (s.get(i + 1 + len) == Some(&b'\'')).then_some(i + 1 + len + 1)?
            }
        };
        return Some(n);
    }
    let mut n = i + 1;
    while n < s.len() {
        match s[n] {
            b'\\' => n += 2,
            b'"' => return Some(n + 1),
            _ => n += 1,
        }
    }
    Some(s.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrapper_never_turns_a_failure_into_success() {
        assert_eq!(exit_byte(Some(0)), 0);
        assert_eq!(exit_byte(Some(101)), 101);
        assert_eq!(exit_byte(Some(256)), 1);
        assert_eq!(exit_byte(Some(0xC000_0100u32 as i32)), 1);
        assert_eq!(exit_byte(None), 1);
    }

    #[test]
    fn rustcs_closing_lines_are_not_errors() {
        assert!(summary("aborting due to 1 previous error"));
        assert!(summary(
            "aborting due to 12 previous errors; 3 warnings emitted"
        ));
        assert!(summary("1 warning emitted"));
        assert!(summary("3 warnings emitted"));
        assert!(!summary("mismatched types"));
        assert!(!summary("unused variable: `warning emitted`"));
    }

    #[test]
    fn recipe_survives_its_file() {
        let r = Recipe {
            rustc: "C:\\rust\\rustc.exe".into(),
            cwd: "C:\\app".into(),
            args: vec!["--crate-name".into(), "app".into(), "a\\b\nc".into()],
            env: vec![
                ("OUT_DIR".into(), "/t/out".into()),
                ("X".into(), "a=b".into()),
            ],
        };
        assert_eq!(Recipe::parse(&r.text()), Some(r));
        assert_eq!(Recipe::parse("junk"), None);
        assert_eq!(Recipe::parse("R x\n"), None);
    }

    #[test]
    fn formatting_and_comments_are_not_code() {
        // Spacing after the last token on a line, and comments where code
        // keeps its place: nothing moved.
        let a = "fn f(x: u8) -> u8 {\n    x + 1 // note\n}\n";
        let b = "fn f(x: u8) -> u8 {\n    x + 1 /* other */   \r\n}";
        assert_eq!(code_hash(a), code_hash(b));
        assert_ne!(code_hash(a), code_hash("fn f(x: u8) -> u8 {\n    x + 2\n}"));
        assert_ne!(code_hash(a), code_hash("fn f(x: u8) -> u8 {\n    x - 1\n}"));
        assert_ne!(code_hash("fn a b"), code_hash("fn ab"));
    }

    #[test]
    fn the_standard_build_script_is_known_however_written() {
        let std = "fn main() {
    wisp_build::run();
}
";
        assert_eq!(
            hash(std, false),
            hash("fn main() { wisp_build::run(); }", false)
        );
        assert_ne!(hash(std, false), hash("fn main() { other::run(); }", false));
    }

    #[test]
    fn moved_code_is_code() {
        // `line!()`, `column!()` and panic locations would be stale.
        let h = code_hash;
        assert_ne!(h("fn f() { panic!() }"), h("fn f() {\n    panic!()\n}"));
        assert_ne!(h("// one\nfn f() {}"), h("// one\n// two\nfn f() {}"));
        assert_ne!(h("fn f() { line!() }"), h("fn f() {  line!() }"));
        assert_ne!(h("let s = \"a\nb\"; x"), h("let s = \"a\nb\";\nx"));
    }

    #[test]
    fn literals_and_docs_are_code() {
        let h = code_hash;
        assert_ne!(h("let s = \"a b\";"), h("let s = \"a  b\";"));
        assert_ne!(h("let s = \"// not a comment\";"), h("let s = \"\";"));
        assert_ne!(h("let s = r#\"a \" b\"#;"), h("let s = r#\"a \" c\"#;"));
        assert_ne!(h("/// doc a\nfn f() {}"), h("/// doc b\nfn f() {}"));
        assert_ne!(h("//! doc a\n"), h("//! doc b\n"));
        assert_eq!(h("//// plain\nfn f() {}"), h("//// other\nfn f() {}"));
        assert_ne!(h("let c = 'a';"), h("let c = 'b';"));
        assert_ne!(h("let c = '\\n';"), h("let c = '\\t';"));
        // A lifetime is not the start of a character.
        assert_eq!(
            h("fn f<'a>(x: &'a str) {} // x"),
            h("fn f<'a>(x: &'a str) {}")
        );
        assert_ne!(h("fn f<'a>(x: &'a str) {}"), h("fn f<'b>(x: &'b str) {}"));
        // Nested block comments end where the outer one does.
        assert_eq!(
            h("/* a /* b */ c */ fn f() {}"),
            h("/* x /* y */ z */ fn f() {}")
        );
        assert_eq!(h("let s = \"é\"; // ü"), h("let s = \"é\";"));
    }
}
