//! How the CLI writes to the terminal, in Kinetrix's terms: three inks (bold
//! for the thing itself, plain, dim for metadata), Wisp violet for what can
//! be typed, and one signal color per meaning: green done, yellow needs a
//! look, red failed. A status line always carries its mark and its words as
//! well, so nothing rests on color alone.

use std::io::{self, BufRead, BufReader, IsTerminal, Read};
use std::sync::OnceLock;

/// ANSI styling, when the terminal is known to understand it. The old
/// Windows console only does if the program switches it on, which takes
/// `unsafe`; there we color only inside the terminals that always do.
pub fn color() -> bool {
    static COLOR: OnceLock<bool> = OnceLock::new();
    *COLOR.get_or_init(|| {
        let var = |k| std::env::var_os(k).is_some();
        let vt = !cfg!(windows) || var("WT_SESSION") || var("TERM_PROGRAM") || var("TERM");
        vt && !var("NO_COLOR") && io::stdout().is_terminal()
    })
}

fn paint(code: &str, s: &str) -> String {
    if color() {
        format!("\x1b[{code}m{s}\x1b[0m")
    } else {
        s.to_string()
    }
}

pub fn bold(s: &str) -> String {
    paint("1", s)
}

pub fn dim(s: &str) -> String {
    paint("2", s)
}

/// Wisp violet, for what can be typed: a command, an answer's mark.
pub fn accent(s: &str) -> String {
    paint("38;2;137;108;224", s)
}

/// The signal colors are the terminal's own, so they suit its theme.
fn green(s: &str) -> String {
    paint("32", s)
}

fn yellow(s: &str) -> String {
    paint("33", s)
}

fn red(s: &str) -> String {
    paint("31", s)
}

/// Something finished: `  ✓ Created my-app from the demo template.`
pub fn done(msg: &str) {
    println!("  {} {msg}", green("✓"));
}

/// Something that needs a look but stopped nothing.
pub fn warn(msg: &str) {
    println!("  {} {msg}", yellow("!"));
}

/// [`warn`] on stderr, for a warning that must not mix into output a script
/// reads. Lines after the first are indented under it.
pub fn warn_err(msg: &str) {
    let mut lines = msg.lines();
    eprintln!("  {} {}", yellow("!"), lines.next().unwrap_or(""));
    for line in lines {
        eprintln!("    {line}");
    }
}

/// A step under way: `  › Building`.
pub fn step(msg: &str) {
    println!("  {} {msg}", dim("›"));
}

/// A file that changed, and what was done about it.
pub fn changed(path: &str, what: &str) {
    if what.is_empty() {
        println!("  {} {path}", dim("~"));
    } else {
        println!("  {} {path}  {}", dim("~"), dim(what));
    }
}

/// A failure, told in parts: what happened on the first line, then the
/// reason or what to do, indented under it.
pub fn failed(msg: &str) {
    let mut lines = msg.lines();
    eprintln!("  {} {}", red("✗"), bold(lines.next().unwrap_or("")));
    for line in lines {
        eprintln!("    {line}");
    }
}

/// Calls `f` with each line a child process writes, without its line ending,
/// until the pipe closes. Bytes that are not UTF-8 are replaced rather than
/// ending the read: a pipe nobody reads fails the child's next write.
pub fn each_line(from: impl Read, mut f: impl FnMut(&str)) {
    let mut from = BufReader::new(from);
    let mut line = Vec::new();
    while from.read_until(b'\n', &mut line).is_ok_and(|n| n > 0) {
        f(String::from_utf8_lossy(&line).trim_end_matches(['\n', '\r']));
        line.clear();
    }
}

/// The brand line that opens `wisp new` and `wisp --help`.
pub fn banner() -> String {
    format!(
        "{}  {}",
        bold(&accent("Wisp")),
        dim("A fast, fun web framework for Rust")
    )
}
