//! Runs `cargo build` and reads its JSON messages: the executable it built
//! and the rendered compiler errors (for the browser overlay).

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

pub struct Build {
    pub ok: bool,
    pub exe: Option<PathBuf>,
    /// Rendered errors without terminal colors.
    pub errors: String,
}

pub fn build(root: &Path, release: bool) -> Build {
    let mut cmd = Command::new("cargo");
    cmd.args(["build", "--message-format=json-diagnostic-rendered-ansi"]);
    if release {
        cmd.arg("--release");
    }
    // stderr stays on the terminal: cargo's progress lines and build-script output.
    cmd.current_dir(root).stdout(Stdio::piped()).stderr(Stdio::inherit());
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => return Build { ok: false, exe: None, errors: format!("could not run cargo: {e}") },
    };

    let mut exe = None;
    let mut errors = String::new();
    let stdout = BufReader::new(child.stdout.take().expect("stdout is piped"));
    for line in stdout.lines().map_while(Result::ok) {
        if line.contains("\"reason\":\"compiler-message\"") {
            // Children carry "rendered":null; the diagnostic's own text is the
            // only string-valued "rendered".
            if let Some(text) = json_string_after(&line, "\"rendered\":\"") {
                eprint!("{text}");
                if line.contains("\"level\":\"error\"") {
                    errors.push_str(&strip_ansi(&text));
                }
            }
        } else if line.contains("\"reason\":\"compiler-artifact\"") && line.contains("\"kind\":[\"bin\"]") {
            exe = json_string_after(&line, "\"executable\":\"").map(PathBuf::from).or(exe);
        }
    }
    let ok = child.wait().is_ok_and(|s| s.success());
    if !ok && errors.is_empty() {
        errors.push_str("cargo build failed; see the terminal for details");
    }
    Build { ok, exe, errors }
}

/// The JSON string that starts right after the last occurrence of `marker`
/// (which ends with the opening quote), unescaped.
fn json_string_after(line: &str, marker: &str) -> Option<String> {
    let start = line.rfind(marker)? + marker.len();
    let mut out = String::new();
    let mut chars = line[start..].chars();
    while let Some(c) = chars.next() {
        match c {
            '"' => return Some(out),
            '\\' => match chars.next()? {
                'n' => out.push('\n'),
                't' => out.push('\t'),
                'r' => out.push('\r'),
                'b' => out.push('\u{8}'),
                'f' => out.push('\u{c}'),
                'u' => {
                    let hex: String = chars.by_ref().take(4).collect();
                    let mut code = u32::from_str_radix(&hex, 16).ok()?;
                    if (0xd800..0xdc00).contains(&code) {
                        // Surrogate pair: \uD83D\uDE00.
                        let low: String = chars.by_ref().skip(2).take(4).collect();
                        let low = u32::from_str_radix(&low, 16).ok()?;
                        code = 0x10000 + ((code - 0xd800) << 10) + (low - 0xdc00);
                    }
                    out.push(char::from_u32(code).unwrap_or('\u{fffd}'));
                }
                other => out.push(other), // \" \\ \/
            },
            c => out.push(c),
        }
    }
    None
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
        let line = r#"{"reason":"compiler-artifact","target":{"kind":["bin"]},"executable":"C:\\app\\target\\debug\\demo.exe","fresh":false}"#;
        assert_eq!(json_string_after(line, "\"executable\":\"").unwrap(), r"C:\app\target\debug\demo.exe");
        let msg = r#"{"message":{"children":[{"rendered":null}],"level":"error","rendered":"error: \u001b[1mbad\u001b[0m \"x\" \ud83d\ude00\n"}}"#;
        let r = json_string_after(msg, "\"rendered\":\"").unwrap();
        assert_eq!(strip_ansi(&r), "error: bad \"x\" 😀\n");
    }
}
