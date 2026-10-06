//! Runs `cargo build` and reads its JSON messages: the executable it built
//! and the rendered compiler errors (for the terminal and the browser).
//!
//! The errors are tidied on the way through. Paths are made relative to the
//! app, the generated modules' names (`page_3::Data`) are dropped, and an
//! error the compiler found in generated code that came from a template is
//! told against the template's own line, which is the one to fix.

use crate::{git_head, term};
use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use wisp_shared::json::{self, Json};

pub struct Build {
    pub ok: bool,
    pub exe: Option<PathBuf>,
    /// Rendered errors without terminal colors.
    pub errors: String,
    /// How many errors, and where the first one is (`src/routes/+page.rs`, 7).
    pub count: usize,
    pub first: Option<(String, usize)>,
}

/// The version of the `wisp` crate the app is locked to, from its Cargo.lock.
fn locked_wisp(lock: &str) -> Option<&str> {
    let mut lines = lock.lines();
    lines.find(|l| matches!(l.trim_end(), "name = \"wisp-web-rt\"" | "name = \"wisp\""))?;
    lines
        .next()?
        .trim_end()
        .strip_prefix("version = \"")?
        .strip_suffix('"')
}

fn triple(v: &str) -> Option<[u32; 3]> {
    let mut it = v.split(['.', '-', '+']).map(|n| n.parse().ok());
    Some([it.next()??, it.next()??, it.next()??])
}

/// The `path` of the app's `wisp` dependency, as its Cargo.toml writes it
/// (`wisp = { path = "../wisp/crates/wisp" }`, or under `[dependencies.wisp]`).
fn wisp_path(toml: &str) -> Option<&str> {
    let mut section = "";
    for line in toml.lines().map(str::trim) {
        if let Some(head) = line.strip_prefix('[') {
            section = head.trim_end_matches(']').trim();
            continue;
        }
        let own = matches!(
            section,
            "dependencies.wisp" | "dev-dependencies.wisp" | "build-dependencies.wisp"
        );
        let deps = matches!(
            section,
            "dependencies" | "dev-dependencies" | "build-dependencies"
        );
        let value = if own {
            Some(line)
        } else if deps {
            line.strip_prefix("wisp")
                .and_then(|r| r.trim_start().strip_prefix('='))
        } else {
            None
        };
        if let Some(path) = value.and_then(path_value) {
            return Some(path);
        }
    }
    None
}

/// `x` in `path = "x"`, wherever in `s`.
fn path_value(s: &str) -> Option<&str> {
    let mut from = 0;
    while let Some(i) = s[from..].find("path") {
        let at = from + i;
        from = at + 4;
        let word = s[..at]
            .chars()
            .next_back()
            .is_some_and(|c| c.is_alphanumeric() || c == '-' || c == '_');
        if word {
            continue;
        }
        let value = s[from..].trim_start().strip_prefix('=')?.trim_start();
        let quote = value.chars().next().filter(|&c| c == '"' || c == '\'')?;
        return value[1..].split(quote).next();
    }
    None
}

/// The framework checkout (the folder with `.git`) the app's `wisp` path
/// dependency is in, if it is one.
fn path_checkout(root: &Path) -> Option<PathBuf> {
    let toml = std::fs::read_to_string(root.join("Cargo.toml")).ok()?;
    let dep = root.join(wisp_path(&toml)?).canonicalize().ok()?;
    let repo = dep.ancestors().find(|d| d.join(".git").exists())?;
    Some(repo.to_path_buf())
}

/// The commits of a build stamp (`1234-abc1234`, see [`git_head::stamp`]).
fn commits(stamp: &str) -> Option<u32> {
    stamp.split('-').next()?.parse().ok()
}

/// What the app's `wisp` is: its locked version, and when it is used by path
/// the checkout and that checkout's build stamp (empty when unknown).
struct App {
    version: String,
    checkout: Option<PathBuf>,
    stamp: String,
}

/// `None` when the app has no Cargo.lock yet, or none that says `wisp`.
fn app_wisp(root: &Path) -> Option<App> {
    let lock = std::fs::read_to_string(root.join("Cargo.lock")).ok()?;
    let version = locked_wisp(&lock)?.to_string();
    let checkout = path_checkout(root);
    let stamp = checkout.as_deref().map(git_head::stamp).unwrap_or_default();
    Some(App {
        version,
        checkout,
        stamp,
    })
}

/// What to say when this CLI is older than the app's `wisp`, if it is: that
/// is a higher version, or the same one used by path from a checkout with
/// more commits. `cli` is the CLI's version and build stamp.
fn outdated(cli: (&str, &str), app: &App) -> Option<String> {
    let same = triple(&app.version) == triple(cli.0);
    let ahead = same
        && app.checkout.is_some()
        && matches!((commits(&app.stamp), commits(cli.1)), (Some(a), Some(c)) if a > c);
    if triple(&app.version) <= triple(cli.0) && !ahead {
        return None;
    }
    let tell = |version: &str, stamp: &str| match stamp.is_empty() {
        true => version.to_string(),
        false => format!("{version}, build {stamp}"),
    };
    let fix = match &app.checkout {
        Some(dir) => {
            let dir = dir.display().to_string();
            let dir = dir.strip_prefix(r"\\?\").unwrap_or(&dir);
            format!("cargo install --path {dir}/crates/wisp-cli --force")
        }
        None => format!("{} --force", crate::dep::install()),
    };
    Some(format!(
        "The wisp CLI is older than this app's wisp crate.\nCLI {}, app {}.\nUpdate it: {fix}\nWISP_NO_UPDATE_CHECK=1 silences this.",
        tell(cli.0, cli.1),
        tell(&app.version, &app.stamp),
    ))
}

/// Before an app command: warns on stderr when the app's wisp is newer than
/// this CLI (its templates and checks may not know it), and at a terminal
/// asks whether to go on, `Err` for no. Without a terminal (CI, a pipe) it
/// never waits. Silent when nothing can be told and with
/// `WISP_NO_UPDATE_CHECK=1`. No network.
pub fn check_updated(root: &Path) -> Result<(), String> {
    if std::env::var_os("WISP_NO_UPDATE_CHECK").is_some_and(|v| !v.is_empty() && v != "0") {
        return Ok(());
    }
    let cli = (env!("CARGO_PKG_VERSION"), env!("WISP_CLI_STAMP"));
    let Some(msg) = app_wisp(root).and_then(|app| outdated(cli, &app)) else {
        return Ok(());
    };
    term::warn_err(&msg);
    let tty = std::io::stdin().is_terminal() && std::io::stderr().is_terminal();
    if !tty || std::env::var_os("CI").is_some() {
        return Ok(());
    }
    eprint!("  Continue anyway? [y/N] ");
    let _ = std::io::stderr().flush();
    let mut answer = String::new();
    let _ = std::io::stdin().read_line(&mut answer);
    match answer.trim().to_ascii_lowercase().as_str() {
        "y" | "yes" => Ok(()),
        _ => Err("Stopped: update the CLI, then run this again.".into()),
    }
}

/// `quiet` leaves out cargo's own progress lines, for rebuilds whose
/// outcome `wisp dev` reports itself.
pub fn build(root: &Path, release: bool, quiet: bool) -> Build {
    build_for(root, release, quiet, &[], &[])
}

/// What a build does to `WISP_BASE`, the base path compiled in.
#[derive(Debug, PartialEq)]
enum BaseEnv {
    Keep,
    Set(String),
    Unset,
}

/// A release build's base is the environment's, else Cargo.toml's
/// (`[package.metadata.wisp] base`). `wisp dev` serves at `/`: the variable
/// unset, not set empty, since cargo reruns `wisp-shared`'s build script
/// (and rebuilds every Wisp crate) when it goes from unset to empty, and a
/// plain `cargo build` or the editor's `cargo check` leaves it unset.
fn base_env(release: bool, set: bool, app_base: impl FnOnce() -> Option<String>) -> BaseEnv {
    match (release, set) {
        (true, true) | (false, false) => BaseEnv::Keep,
        (true, false) => app_base().map_or(BaseEnv::Keep, BaseEnv::Set),
        (false, true) => BaseEnv::Unset,
    }
}

/// [`build`] with more of cargo's arguments, such as `--target
/// wasm32-unknown-unknown` (whose "executable" is the `.wasm` file), and
/// `env` set for cargo.
pub fn build_for(
    root: &Path,
    release: bool,
    quiet: bool,
    args: &[&str],
    env: &[(&str, &str)],
) -> Build {
    run("build", root, release, quiet, args, env)
}

/// `cargo check` of the app (`wisp check --rust`): its Rust and the code
/// its templates compile to are type-checked, with no code generated and
/// no link, errors told as a build tells them.
pub fn check(root: &Path) -> Build {
    run("check", root, false, false, &[], &[])
}

fn run(
    sub: &str,
    root: &Path,
    release: bool,
    quiet: bool,
    args: &[&str],
    env: &[(&str, &str)],
) -> Build {
    let mut cmd = Command::new("cargo");
    match base_env(release, std::env::var_os("WISP_BASE").is_some(), || {
        wisp_build::app_base(root)
    }) {
        BaseEnv::Keep => {}
        BaseEnv::Set(base) => {
            cmd.env("WISP_BASE", base);
        }
        BaseEnv::Unset => {
            cmd.env_remove("WISP_BASE");
        }
    }
    cmd.envs(env.iter().copied());
    cmd.args([sub, "--message-format=json-diagnostic-rendered-ansi"]);
    cmd.args(args);
    if release {
        cmd.arg("--release");
    }
    if quiet {
        cmd.arg("--quiet");
    }
    // stderr goes to the terminal: cargo's progress and build-script output.
    // Quiet, it is read through instead, to drop the "could not compile"
    // line that `wisp dev` says in its own words.
    let stderr = if quiet {
        Stdio::piped()
    } else {
        Stdio::inherit()
    };
    // The build script leaves its warnings to the CLI, which checked first.
    cmd.env("WISP_CLI", "1");
    cmd.current_dir(root).stdout(Stdio::piped()).stderr(stderr);
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            let errors = format!(
                "Could not run cargo: {e}.\nInstall Rust from https://rustup.rs, and open a new terminal."
            );
            // Callers say "the errors are above"; quiet ones print `errors`.
            if !quiet {
                eprintln!("{errors}");
            }
            return Build {
                ok: false,
                exe: None,
                errors,
                count: 1,
                first: None,
            };
        }
    };

    let stderr = child.stderr.take().map(|err| {
        std::thread::spawn(move || {
            term::each_line(err, |line| {
                if !line.starts_with("error: could not compile") {
                    eprintln!("{line}");
                }
            });
        })
    });
    // An app with more than one binary (a `src/bin/seed.rs` beside its
    // main.rs) is the one named after the package.
    let package = package_name(root);
    let mut b = Build {
        ok: false,
        exe: None,
        errors: String::new(),
        count: 0,
        first: None,
    };
    term::each_line(child.stdout.take().expect("stdout is piped"), |line| {
        let Ok(msg) = json::parse(line) else {
            return;
        };
        match msg.str("reason") {
            Some("compiler-message") => {
                let Some(diag) = msg.get("message") else {
                    return;
                };
                if let Some(shown) = diagnostic(&mut b, diag, root) {
                    eprint!("{shown}");
                }
            }
            Some("compiler-artifact") => {
                let kinds = msg.get("target").and_then(|t| t.get("kind"));
                let bin = kinds.is_some_and(|k| k.items().any(|k| k.as_str() == Some("bin")));
                let Some(exe) = msg.str("executable").filter(|_| bin).map(PathBuf::from) else {
                    return;
                };
                let named = exe.file_stem().and_then(|s| s.to_str()) == package.as_deref();
                if named || b.exe.is_none() {
                    b.exe = Some(exe);
                }
            }
            _ => {}
        }
    });
    b.ok = child.wait().is_ok_and(|s| s.success());
    // All of cargo's own output is out before `wisp dev` says anything.
    if let Some(t) = stderr {
        let _ = t.join();
    }
    if !b.ok && b.errors.is_empty() {
        b.errors.push_str(&format!(
            "cargo {sub} failed. The terminal has the details."
        ));
    }
    b
}

/// A diagnostic of rustc (`message` of cargo's `compiler-message`, or a line
/// of rustc's own JSON): counted into `b` when it is an error, and the text
/// to show for it, retold against the template when it came from one.
pub fn diagnostic(b: &mut Build, diag: &Json, root: &Path) -> Option<String> {
    let text = tidy(diag.str("rendered")?);
    let plain = strip_ansi(&text);
    let retold = from_template(diag, &plain, root);
    let shown = match &retold {
        Some(t) if term::color() => colorize(t),
        Some(t) => t.clone(),
        None if term::color() => text,
        None => plain.clone(),
    };
    if diag.str("level") == Some("error") {
        let plain = retold.unwrap_or(plain);
        b.count += 1;
        if b.first.is_none() {
            b.first = location(&plain);
        }
        b.errors.push_str(&plain);
    }
    Some(shown)
}

/// The `name` under `[package]` in the app's Cargo.toml.
pub fn package_name(root: &Path) -> Option<String> {
    let toml = std::fs::read_to_string(root.join("Cargo.toml")).ok()?;
    let mut in_package = false;
    for line in toml.lines().map(str::trim) {
        if line.starts_with('[') {
            in_package = line == "[package]";
        } else if in_package
            && let Some(value) = line
                .strip_prefix("name")
                .and_then(|r| r.trim_start().strip_prefix('='))
        {
            let value = value.trim_start();
            let quote = value.chars().next().filter(|&c| c == '"' || c == '\'')?;
            return value[1..].split(quote).next().map(String::from);
        }
    }
    None
}

/// `path` relative to the working directory, with `/` separators, if it is
/// inside it. Anything else comes back as it was.
pub fn relative(path: &str) -> String {
    let Ok(dir) = std::env::current_dir() else {
        return path.to_string();
    };
    let dir = dir.to_string_lossy();
    match strip_prefix_ignoring_case(path, &dir) {
        Some(rest) if rest.starts_with(['/', '\\']) => rest[1..].replace('\\', "/"),
        _ => path.to_string(),
    }
}

fn strip_prefix_ignoring_case<'a>(s: &'a str, prefix: &str) -> Option<&'a str> {
    let head = s.get(..prefix.len())?;
    let same = if cfg!(windows) {
        head.eq_ignore_ascii_case(prefix)
    } else {
        head == prefix
    };
    same.then(|| &s[prefix.len()..])
}

/// Relative paths after every `-->`, and no generated module names.
fn tidy(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for line in text.split_inclusive('\n') {
        match line.find("--> ") {
            Some(i) => {
                // The path starts after the arrow and any color codes.
                let mut start = i + 4;
                while line[start..].starts_with('\x1b') {
                    start += line[start..]
                        .find('m')
                        .map_or(line.len() - start, |m| m + 1);
                }
                let end = line[start..]
                    .find(['\x1b', '\r', '\n'])
                    .map_or(line.len(), |e| start + e);
                out.push_str(&line[..start]);
                out.push_str(&relative(&line[start..end]).replace('\\', "/"));
                out.push_str(&line[end..]);
            }
            None => out.push_str(line),
        }
    }
    strip_generated_modules(&out)
}

/// `page_3::Data` → `Data`, `__wisp::App` → `App`: the modules wisp-build
/// generates mean nothing to the person reading the error.
fn strip_generated_modules(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    let mut copied = 0;
    while i < b.len() {
        let at_word = i == 0 || !(b[i - 1].is_ascii_alphanumeric() || b[i - 1] == b'_');
        // The names are ASCII: never look from the middle of a character.
        if at_word && s.is_char_boundary(i) {
            let rest = &s[i..];
            let plain = ["__wisp::", "__call::", "__mods::"]
                .iter()
                .find_map(|p| rest.strip_prefix(p));
            let skip = if let Some(r) = plain {
                Some(rest.len() - r.len())
            } else {
                [
                    "tpl_page_",
                    "tpl_layout_",
                    "tpl_error_",
                    "tpl_component_",
                    "page_",
                    "layout_",
                    "server_",
                ]
                .iter()
                .find_map(|p| {
                    let r = rest.strip_prefix(p)?;
                    let digits = r.bytes().take_while(u8::is_ascii_digit).count();
                    (digits > 0 && r[digits..].starts_with("::")).then(|| p.len() + digits + 2)
                })
            };
            if let Some(n) = skip {
                out.push_str(&s[copied..i]);
                i += n;
                copied = i;
                continue;
            }
        }
        i += 1;
    }
    out.push_str(&s[copied..]);
    out
}

/// A diagnostic (cargo's JSON `message`) whose primary span is in the
/// generated `wisp.rs`, on a line that came from a template (it ends with
/// `// src/routes/+page.wisp:2`), retold against the template: its path and
/// line, its own text with the same part underlined, and the notes. `plain`
/// is its rendered text, for the heading. `None` for anything else.
fn from_template(diag: &Json, plain: &str, root: &Path) -> Option<String> {
    let span = diag
        .get("spans")?
        .items()
        .find(|s| s.get("is_primary") == Some(&Json::Bool(true)))?;
    if !span.str("file_name")?.ends_with("wisp.rs") {
        return None;
    }
    let first = span.get("text")?.items().next()?;
    let code = first.str("text")?;
    let (_, origin) = code.rsplit_once("// ")?;
    let (template, line) = origin.trim().rsplit_once(':')?;
    if !template.ends_with(".wisp") {
        return None;
    }
    let line: usize = line.parse().ok()?;
    let source = wisp_build::read_source(&root.join(template)).ok()?;
    let text = source.lines().nth(line.checked_sub(1)?)?.trim_end();

    // What is underlined (columns count characters, from 1), and its label.
    let (from, to) = (num(first, "highlight_start")?, num(first, "highlight_end")?);
    let underlined: String = code
        .chars()
        .skip(from.saturating_sub(1))
        .take(to.saturating_sub(from))
        .collect();
    let label = span.str("label").unwrap_or("");
    let column = (!underlined.is_empty())
        .then(|| text.find(&underlined))
        .flatten();

    let num = line.to_string();
    let pad = " ".repeat(num.len());
    let mut out = plain.lines().next()?.to_string();
    out.push('\n');
    match column {
        Some(c) => out.push_str(&format!("{pad}--> {template}:{line}:{}\n", c + 1)),
        None => out.push_str(&format!("{pad}--> {template}:{line}\n")),
    }
    out.push_str(&format!("{pad} |\n{num} | {text}\n"));
    match column {
        Some(c) => out.push_str(&format!(
            "{pad} | {}{} {label}\n",
            " ".repeat(text[..c].chars().count()),
            "^".repeat(underlined.chars().count())
        )),
        None if !label.is_empty() => out.push_str(&format!("{pad} = {label}\n")),
        None => {}
    }
    // The notes and help that stand alone, as rustc writes them under the code.
    let notes: Vec<String> = diag
        .get("children")
        .into_iter()
        .flat_map(Json::items)
        .filter(|c| c.get("spans").is_none_or(|s| s.items().next().is_none()))
        .filter_map(|c| Some(format!("= {}: {}", c.str("level")?, c.str("message")?)))
        .collect();
    if !notes.is_empty() {
        out.push_str(&format!("{pad} |\n"));
    }
    for t in notes {
        out.push_str(&format!("{pad} {t}\n"));
    }
    out.push('\n');
    Some(out)
}

/// rustc's colors, for a diagnostic `from_template` retold.
fn colorize(diag: &str) -> String {
    const RED: &str = "\x1b[1m\x1b[91m";
    const BLUE: &str = "\x1b[1m\x1b[96m";
    const BOLD: &str = "\x1b[1m";
    const RESET: &str = "\x1b[0m";
    let mut out = String::with_capacity(diag.len() * 2);
    for (i, line) in diag.lines().enumerate() {
        if i == 0 {
            match line.split_once(": ") {
                Some((kind, msg)) => {
                    out.push_str(&format!("{RED}{kind}{RESET}{BOLD}: {msg}{RESET}"))
                }
                None => out.push_str(line),
            }
        } else if !line.is_empty() {
            // The gutter (`-->`, the line number and `|`, or `=`) in blue,
            // the carets and their label in red.
            let gutter_end = match line.find("-->") {
                Some(g) => g + 3,
                None => line.find(['|', '=']).map_or(0, |g| g + 1),
            };
            let (gutter, rest) = line.split_at(gutter_end);
            out.push_str(&format!("{BLUE}{gutter}{RESET}"));
            match rest.find('^') {
                Some(c) => out.push_str(&format!("{}{RED}{}{RESET}", &rest[..c], &rest[c..])),
                None => out.push_str(rest),
            }
        }
        out.push('\n');
    }
    out
}

/// The first `-->` location: the file, and the line.
fn location(diag: &str) -> Option<(String, usize)> {
    let at = diag
        .lines()
        .find_map(|l| l.trim_start().strip_prefix("--> "))?;
    let mut parts = at.rsplitn(3, ':');
    let (a, b) = (parts.next()?, parts.next()?);
    // `file:line:col` or `file:line`
    match parts.next() {
        Some(file) => Some((file.to_string(), b.parse().ok()?)),
        None => Some((b.to_string(), a.parse().ok()?)),
    }
}

/// A whole number member of a JSON object.
fn num(v: &Json, key: &str) -> Option<usize> {
    match v.get(key)? {
        Json::Num(n) => n.parse().ok(),
        _ => None,
    }
}

pub fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            // CSI sequence: ESC [ params letter
            for c in chars.by_ref() {
                if c.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dev_leaves_the_base_unset_so_wisp_crates_stay_built() {
        let none = || None;
        assert_eq!(base_env(false, false, none), BaseEnv::Keep);
        assert_eq!(base_env(false, true, none), BaseEnv::Unset);
        assert_eq!(base_env(true, true, none), BaseEnv::Keep);
        assert_eq!(base_env(true, false, none), BaseEnv::Keep);
        let app = || Some("/app".to_string());
        assert_eq!(base_env(true, false, app), BaseEnv::Set("/app".into()));
    }

    #[test]
    fn stale_cli() {
        let lock = "[[package]]\nname = \"wisp\"\nversion = \"0.2.1\"\n";
        assert_eq!(locked_wisp(lock), Some("0.2.1"));
        assert_eq!(locked_wisp(&lock.replace('\n', "\r\n")), Some("0.2.1"));
        assert!(triple("0.2.1") > triple("0.1.0"));
        assert!(triple("0.10.0") > triple("0.9.9"));
        assert_eq!(
            locked_wisp("name = \"wisp-build\"\nversion = \"9.0.0\"\n"),
            None
        );
        assert_eq!(locked_wisp("name = \"wisp\""), None);
    }

    #[test]
    fn finds_the_wisp_path_dependency() {
        let dep = "[package]\nname = \"a\"\n\n[dependencies]\nwisp-build = { path = \"no\" }\nwisp = { version = \"0.1\", path = \"../w/crates/wisp\", features = [] }\n";
        assert_eq!(wisp_path(dep), Some("../w/crates/wisp"));
        let table = "[dependencies.wisp]\nfeatures = []\npath = '../w'\n";
        assert_eq!(wisp_path(table), Some("../w"));
        assert_eq!(wisp_path("[dependencies]\nwisp = \"0.1\"\n"), None);
        assert_eq!(wisp_path("[dependencies]\nwisp.workspace = true\n"), None);
        assert_eq!(
            wisp_path("[dependencies]\nwisp = { git = \"x\", subpath = \"y\" }\n"),
            None
        );
        assert_eq!(wisp_path("[package]\nwisp = { path = \"no\" }\n"), None);
    }

    fn app(version: &str, checkout: bool, stamp: &str) -> App {
        App {
            version: version.into(),
            checkout: checkout.then(|| PathBuf::from("/src/wisp")),
            stamp: stamp.into(),
        }
    }

    #[test]
    fn decides_whether_the_cli_is_outdated() {
        let cli = ("0.1.0", "10-aaaaaaa");
        // Equal, or the CLI is newer: quiet.
        assert_eq!(outdated(cli, &app("0.1.0", false, "")), None);
        assert_eq!(outdated(cli, &app("0.0.9", true, "99-b")), None);
        assert_eq!(outdated(cli, &app("0.1.0", true, "10-aaaaaaa")), None);
        // A newer version from the registry: the plain fix.
        let told = outdated(cli, &app("0.2.0", false, "")).unwrap();
        assert!(told.contains("CLI 0.1.0, build 10-aaaaaaa"), "{told}");
        assert!(told.contains("app 0.2.0"), "{told}");
        assert!(
            told.contains(&format!("{} --force", crate::dep::install())),
            "{told}"
        );
        assert!(told.contains("WISP_NO_UPDATE_CHECK=1"), "{told}");
        // The same version by path: by commits.
        let told = outdated(cli, &app("0.1.0", true, "12-bbbbbbb")).unwrap();
        assert!(
            told.contains("cargo install --path /src/wisp/crates/wisp-cli --force"),
            "{told}"
        );
        assert_eq!(outdated(cli, &app("0.1.0", true, "9-bbbbbbb")), None);
        // Another hash at the same count, or no count known: nothing to say.
        assert_eq!(outdated(cli, &app("0.1.0", true, "10-bbbbbbb")), None);
        assert_eq!(outdated(cli, &app("0.1.0", true, "")), None);
        assert_eq!(outdated(("0.1.0", ""), &app("0.1.0", true, "5-b")), None);
        // From the registry the same version has no stamp to compare.
        assert_eq!(outdated(cli, &app("0.1.0", false, "99-b")), None);
        // A newer version is told even with no stamps.
        assert!(outdated(("0.1.0", ""), &app("0.1.1", true, "")).is_some());
    }

    #[test]
    fn reads_what_the_app_locks() {
        let dir = std::env::temp_dir().join(format!("wisp-lock-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        assert!(app_wisp(&dir).is_none());
        let lock = "[[package]]\nname = \"wisp\"\nversion = \"3.0.0\"\n";
        std::fs::write(dir.join("Cargo.lock"), lock).unwrap();
        let app = app_wisp(&dir).unwrap();
        assert_eq!(app.version, "3.0.0");
        assert!(app.checkout.is_none() && app.stamp.is_empty());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_stamp_is_commits_then_hash() {
        assert_eq!(commits("1234-abc1234"), Some(1234));
        assert_eq!(commits(""), None);
        assert_eq!(commits("-abc"), None);
        assert_eq!(git_head::stamp(Path::new("/no/such/checkout")), "");
    }

    #[test]
    fn reads_cargo_json() {
        let line = r#"{"reason":"compiler-artifact","target":{"kind":["bin"]},"executable":"C:\\app\\target\\debug\\demo.exe","fresh":false,"n":-1.5e3}"#;
        let v = json::parse(line).unwrap();
        assert_eq!(v.str("executable"), Some(r"C:\app\target\debug\demo.exe"));
        let kind = v.get("target").and_then(|t| t.get("kind")).unwrap();
        assert_eq!(kind.items().next(), Some(&Json::Str("bin".into())));
        assert_eq!(v.get("fresh"), Some(&Json::Bool(false)));
        let msg = r#"{"message":{"children":[{"rendered":null}],"level":"error","rendered":"error: \u001b[1mbad\u001b[0m \"x\" \ud83d\ude00\n"}}"#;
        let diag = json::parse(msg).unwrap();
        let r = diag.get("message").and_then(|m| m.str("rendered")).unwrap();
        assert_eq!(strip_ansi(r), "error: bad \"x\" 😀\n");
    }

    #[test]
    fn reads_the_package_name() {
        let root = std::env::temp_dir().join(format!("wisp-package-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let toml = "[workspace]\nname = \"no\"\n\n[package]\nversion = \"0.1.0\"\nname = 'my-app' # the app\n\n[[bin]]\nname = \"seed\"\n";
        std::fs::write(root.join("Cargo.toml"), toml).unwrap();
        assert_eq!(package_name(&root).as_deref(), Some("my-app"));
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn drops_generated_module_names() {
        assert_eq!(
            strip_generated_modules("no field `count` on type `&page_1::Data`"),
            "no field `count` on type `&Data`"
        );
        assert_eq!(
            strip_generated_modules("__wisp::layout_12::Data and tpl_page_3::render"),
            "Data and render"
        );
        assert_eq!(
            strip_generated_modules("page_2::tpl_page_2::render and page_2::__call::Loaded"),
            "render and Loaded"
        );
        assert_eq!(
            strip_generated_modules("my_page_1::X page_::Y page_2x::Z"),
            "my_page_1::X page_::Y page_2x::Z"
        );
        // Source lines in an error can hold any text.
        assert_eq!(
            strip_generated_modules("café “page_1::Data” 😀 __wisp::App"),
            "café “Data” 😀 App"
        );
    }

    #[test]
    fn tells_template_errors_against_the_template() {
        let root = std::env::temp_dir().join(format!("wisp-cargo-{}", std::process::id()));
        std::fs::create_dir_all(root.join("src/routes")).unwrap();
        std::fs::write(
            root.join("src/routes/+page.wisp"),
            "<h1>Hi</h1>\n<p>{data.count}</p>\n",
        )
        .unwrap();
        let plain = "error[E0609]: no field `count` on type `&Data`\n  \
                     --> target/debug/build/app-1/out/wisp.rs:55:35\n";
        let diag = json::parse(
            r#"{"children":[{"children":[],"level":"note","message":"available field is: `n`","rendered":null,"spans":[]}],
                "level":"error","message":"no field `count` on type `&Data`","rendered":"…",
                "spans":[{"file_name":"target/debug/build/app-1/out/wisp.rs","is_primary":true,"label":"unknown field",
                  "line_start":55,"column_start":35,"column_end":40,
                  "text":[{"highlight_end":40,"highlight_start":35,"text":"        (&::wisp::rt::Text(&(data.count))).put(&mut __o.body); // src/routes/+page.wisp:2"}]}]}"#,
        )
        .unwrap();
        let told = from_template(&diag, plain, &root).unwrap();
        assert_eq!(
            told,
            "error[E0609]: no field `count` on type `&Data`\n \
             --> src/routes/+page.wisp:2:10\n  \
             |\n\
             2 | <p>{data.count}</p>\n  \
             |          ^^^^^ unknown field\n  \
             |\n  \
             = note: available field is: `n`\n\n"
        );
        assert_eq!(location(&told), Some(("src/routes/+page.wisp".into(), 2)));
        std::fs::remove_dir_all(&root).unwrap();
    }
}
