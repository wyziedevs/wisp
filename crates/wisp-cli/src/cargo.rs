//! Runs `cargo build` and reads its JSON messages: the executable it built
//! and the rendered compiler errors (for the terminal and the browser).
//!
//! The errors are tidied on the way through. Paths are made relative to the
//! app, the generated modules' names (`page_3::Data`) are dropped, and an
//! error the compiler found in generated code that came from a template is
//! told against the template's own line, which is the one to fix.

use crate::term;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

pub struct Build {
    pub ok: bool,
    pub exe: Option<PathBuf>,
    /// Rendered errors without terminal colors.
    pub errors: String,
    /// How many errors, and where the first one is (`src/routes/+page.rs`, 7).
    pub count: usize,
    pub first: Option<(String, usize)>,
}

/// `quiet` leaves out cargo's own progress lines, for rebuilds whose
/// outcome `wisp dev` reports itself.
pub fn build(root: &Path, release: bool, quiet: bool) -> Build {
    build_for(root, release, quiet, None)
}

/// [`build`] for another target, such as `wasm32-unknown-unknown`, whose
/// "executable" is the `.wasm` file.
pub fn build_for(root: &Path, release: bool, quiet: bool, target: Option<&str>) -> Build {
    let mut cmd = Command::new("cargo");
    cmd.args(["build", "--message-format=json-diagnostic-rendered-ansi"]);
    if let Some(t) = target {
        cmd.args(["--target", t]);
    }
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
    cmd.current_dir(root).stdout(Stdio::piped()).stderr(stderr);
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            let errors = format!("Could not run cargo: {e}.");
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
        let Some(msg) = Json::parse(line) else {
            return;
        };
        match msg.str("reason") {
            Some("compiler-message") => {
                let Some(diag) = msg.get("message") else {
                    return;
                };
                let Some(text) = diag.str("rendered") else {
                    return;
                };
                let text = tidy(text);
                let plain = strip_ansi(&text);
                let retold = from_template(diag, &plain, root);
                match &retold {
                    Some(t) if term::color() => eprint!("{}", colorize(t)),
                    Some(t) => eprint!("{t}"),
                    None if term::color() => eprint!("{text}"),
                    None => eprint!("{plain}"),
                }
                if diag.str("level") == Some("error") {
                    let plain = retold.unwrap_or(plain);
                    b.count += 1;
                    if b.first.is_none() {
                        b.first = location(&plain);
                    }
                    b.errors.push_str(&plain);
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
        b.errors
            .push_str("cargo build failed. The terminal has the details.");
    }
    b
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
        if at_word {
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
    let (from, to) = (first.num("highlight_start")?, first.num("highlight_end")?);
    let underlined: String = code.chars().skip(from - 1).take(to - from).collect();
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

/// A JSON value: a line of cargo's `--message-format=json` output, read
/// whole. Numbers are kept as their text.
#[derive(Debug, PartialEq)]
enum Json {
    Null,
    Bool(bool),
    Num(String),
    Str(String),
    Arr(Vec<Json>),
    Obj(Vec<(String, Json)>),
}

impl Json {
    /// `s` if it is one JSON value (and nothing after it).
    fn parse(s: &str) -> Option<Json> {
        let mut chars = s.trim().chars().peekable();
        let v = Json::value(&mut chars)?;
        chars.next().is_none().then_some(v)
    }

    fn value(c: &mut std::iter::Peekable<std::str::Chars>) -> Option<Json> {
        let skip = |c: &mut std::iter::Peekable<std::str::Chars>| {
            while c.next_if(|x| x.is_ascii_whitespace()).is_some() {}
        };
        skip(c);
        let v = match *c.peek()? {
            '{' => {
                c.next();
                let mut m = Vec::new();
                skip(c);
                if c.next_if_eq(&'}').is_none() {
                    loop {
                        skip(c);
                        let Json::Str(k) = Json::value(c)? else {
                            return None;
                        };
                        skip(c);
                        c.next_if_eq(&':')?;
                        m.push((k, Json::value(c)?));
                        skip(c);
                        match c.next()? {
                            ',' => {}
                            '}' => break,
                            _ => return None,
                        }
                    }
                }
                Json::Obj(m)
            }
            '[' => {
                c.next();
                let mut a = Vec::new();
                skip(c);
                if c.next_if_eq(&']').is_none() {
                    loop {
                        a.push(Json::value(c)?);
                        skip(c);
                        match c.next()? {
                            ',' => {}
                            ']' => break,
                            _ => return None,
                        }
                    }
                }
                Json::Arr(a)
            }
            '"' => {
                c.next();
                Json::Str(Json::string(c)?)
            }
            _ => {
                let word: String =
                    std::iter::from_fn(|| c.next_if(|x| !",]} \t\r\n".contains(*x))).collect();
                match word.as_str() {
                    "null" => Json::Null,
                    "true" => Json::Bool(true),
                    "false" => Json::Bool(false),
                    n if n.parse::<f64>().is_ok() => Json::Num(word),
                    _ => return None,
                }
            }
        };
        skip(c);
        Some(v)
    }

    /// The rest of a string whose opening quote has been read, unescaped.
    fn string(c: &mut std::iter::Peekable<std::str::Chars>) -> Option<String> {
        let mut out = String::new();
        let hex = |c: &mut std::iter::Peekable<std::str::Chars>| {
            let h: String = c.by_ref().take(4).collect();
            u32::from_str_radix(&h, 16).ok()
        };
        loop {
            match c.next()? {
                '"' => return Some(out),
                '\\' => match c.next()? {
                    'n' => out.push('\n'),
                    't' => out.push('\t'),
                    'r' => out.push('\r'),
                    'b' => out.push('\u{8}'),
                    'f' => out.push('\u{c}'),
                    'u' => {
                        let mut code = hex(c)?;
                        if (0xd800..0xdc00).contains(&code) {
                            // A surrogate pair: \uD83D\uDE00.
                            c.next_if_eq(&'\\')?;
                            c.next_if_eq(&'u')?;
                            code = 0x10000 + ((code - 0xd800) << 10) + (hex(c)? - 0xdc00);
                        }
                        out.push(char::from_u32(code).unwrap_or('\u{fffd}'));
                    }
                    other => out.push(other), // \" \\ \/
                },
                ch => out.push(ch),
            }
        }
    }

    fn get(&self, key: &str) -> Option<&Json> {
        match self {
            Json::Obj(m) => m.iter().rev().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    fn as_str(&self) -> Option<&str> {
        match self {
            Json::Str(s) => Some(s),
            _ => None,
        }
    }

    fn str(&self, key: &str) -> Option<&str> {
        self.get(key)?.as_str()
    }

    fn num(&self, key: &str) -> Option<usize> {
        match self.get(key)? {
            Json::Num(n) => n.parse().ok(),
            _ => None,
        }
    }

    /// The items of an array; nothing for anything else.
    fn items(&self) -> std::slice::Iter<'_, Json> {
        match self {
            Json::Arr(a) => a.iter(),
            _ => [].iter(),
        }
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
    fn reads_cargo_json() {
        let line = r#"{"reason":"compiler-artifact","target":{"kind":["bin"]},"executable":"C:\\app\\target\\debug\\demo.exe","fresh":false,"n":-1.5e3}"#;
        let v = Json::parse(line).unwrap();
        assert_eq!(v.str("executable"), Some(r"C:\app\target\debug\demo.exe"));
        let kind = v.get("target").and_then(|t| t.get("kind")).unwrap();
        assert_eq!(kind.items().next(), Some(&Json::Str("bin".into())));
        assert_eq!(v.get("fresh"), Some(&Json::Bool(false)));
        let msg = r#"{"message":{"children":[{"rendered":null}],"level":"error","rendered":"error: \u001b[1mbad\u001b[0m \"x\" \ud83d\ude00\n"}}"#;
        let diag = Json::parse(msg).unwrap();
        let r = diag.get("message").and_then(|m| m.str("rendered")).unwrap();
        assert_eq!(strip_ansi(r), "error: bad \"x\" 😀\n");
        for bad in ["", "{", "[1,]", "{\"a\" 1}", "tru", "\"x", "{} x"] {
            assert_eq!(Json::parse(bad), None, "{bad}");
        }
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
        let diag = Json::parse(
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
