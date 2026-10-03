//! `wisp fmt`: a `.wisp` file in one layout. The markup's element tree and
//! template blocks are indented two spaces a level, attribute values are
//! double-quoted, a start tag that begins its line sits on it whole or, too
//! long, one attribute a line. The `---` block goes through rustfmt, a
//! `<script>` is indented, a `<style>` gets one declaration a line.
//!
//! Only whitespace between tokens and attribute quotes change, never text,
//! code or the lines of `<pre>` and `<textarea>`. Whatever the formatter is
//! not sure of stays as written: markup that does not balance, a script with
//! template literals, a block rustfmt refuses. The markup formatted must
//! parse to the template it parsed to before, or it is left alone too.

use crate::template::{self, hole_end};
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

/// Columns a start tag may take on one line, its indentation included.
const WIDTH: usize = 100;

const VOID: [&str; 14] = [
    "area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta", "param", "source",
    "track", "wbr",
];

/// The Rust edition of the crate `path` is in, for rustfmt: the nearest
/// `Cargo.toml` above it that names one (`edition.workspace = true` looks
/// further up, to the workspace's), else 2024. As `cargo fmt` formats.
pub fn edition(path: &Path) -> String {
    for dir in path.ancestors() {
        let Ok(toml) = std::fs::read_to_string(dir.join("Cargo.toml")) else {
            continue;
        };
        for line in toml.lines() {
            let Some(rest) = line.trim().strip_prefix("edition") else {
                continue;
            };
            let Some(v) = rest.trim_start().strip_prefix('=') else {
                continue;
            };
            let v = v.trim().trim_matches(['"', '\'']);
            if v.len() == 4 && v.bytes().all(|b| b.is_ascii_digit()) {
                return v.to_string();
            }
        }
    }
    "2024".into()
}

/// `src` formatted: the same file in Wisp's layout, its block as rustfmt
/// formats Rust of `edition`. Idempotent.
pub fn format(src: &str, edition: &str) -> String {
    let (bom, text) = match src.strip_prefix('\u{feff}') {
        Some(t) => ("\u{feff}", t),
        None => ("", src),
    };
    let crlf = text.contains("\r\n");
    let lf = text.replace("\r\n", "\n");
    if lf.contains('\r') {
        return src.to_string();
    }
    let Some(out) = format_lf(&lf, edition) else {
        return src.to_string();
    };
    let out = if crlf { out.replace('\n', "\r\n") } else { out };
    format!("{bom}{out}")
}

/// A file with `\n` line ends formatted; `None` when its block has no end.
fn format_lf(src: &str, edition: &str) -> Option<String> {
    let lines: Vec<&str> = src.split('\n').collect();
    let open = lines.iter().position(|l| !l.trim().is_empty());
    let Some(open) = open.filter(|&o| lines[o].trim() == "---") else {
        return Some(markup(src));
    };
    let close = open + 1 + lines[open + 1..].iter().position(|l| l.trim() == "---")?;
    let block: String = lines[open + 1..close]
        .iter()
        .map(|l| format!("{l}\n"))
        .collect();
    let rust = if block.trim().is_empty() {
        String::new()
    } else {
        rustfmt(&block, edition).unwrap_or(block)
    };
    let rest = lines[close + 1..].join("\n");
    Some(format!("---\n{rust}---\n{}", markup(&rest)))
}

/// The statements and items of a `---` block through rustfmt, inside a
/// function and out again. Every line goes in four spaces deeper and
/// comes out four spaces shallower, so a string's lines stay as they were.
/// `None` when rustfmt is missing or fails.
fn rustfmt(block: &str, edition: &str) -> Option<String> {
    const HEAD: &str = "fn __wisp_fmt() {\n";
    let mut wrapped = String::from(HEAD);
    for l in block.lines() {
        wrapped.push_str("    ");
        wrapped.push_str(l);
        wrapped.push('\n');
    }
    wrapped.push_str("}\n");
    let mut child = Command::new("rustfmt")
        .args(["--edition", edition, "--emit", "stdout"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    // rustfmt reads all its input before it writes: no deadlock.
    child.stdin.take()?.write_all(wrapped.as_bytes()).ok()?;
    let out = child.wait_with_output().ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8(out.stdout).ok()?.replace("\r\n", "\n");
    if text == "fn __wisp_fmt() {}\n" {
        return Some(String::new());
    }
    let inner = text.strip_prefix(HEAD)?.strip_suffix("}\n")?;
    let mut rust = String::with_capacity(inner.len());
    for l in inner.lines() {
        rust.push_str(if l.is_empty() {
            l
        } else {
            l.strip_prefix("    ")?
        });
        rust.push('\n');
    }
    Some(rust)
}

/// The markup formatted, or as it was when it does not scan or would not
/// parse to the same template.
fn markup(m: &str) -> String {
    let format = || {
        let s = Scan::run(m)?;
        let quoted = requote(m, &s.quotes);
        let s = if s.quotes.is_empty() {
            s
        } else {
            Scan::run(&quoted)?
        };
        let out = render(&quoted, &s)?;
        (out == m || same(m, &out)).then_some(out)
    };
    format().unwrap_or_else(|| m.to_string())
}

/// What becomes of a line of markup.
#[derive(Clone, Copy, PartialEq)]
enum Plan {
    /// Starts in text: indented this many levels.
    Indent(usize),
    /// Starts between a start tag's attributes: indented this many levels.
    Attr(usize),
    /// Starts inside a value, a hole, a comment or raw text: as it is.
    Keep,
}

/// A start tag that begins its line.
struct Tag {
    line: usize,
    last_line: usize,
    /// Its `<` and the byte after its `>`.
    start: usize,
    end: usize,
    name_end: usize,
    attrs: Vec<(usize, usize)>,
    self_closing: bool,
    /// No attribute value spans lines.
    joinable: bool,
}

/// The lines of a `<script>` or `<style>` between its tags.
struct Raw {
    first: usize,
    close: usize,
    depth: usize,
    style: bool,
}

#[derive(Default)]
struct Scan {
    plans: Vec<Plan>,
    /// Attribute values to double-quote: their range and new text.
    quotes: Vec<(usize, usize, String)>,
    tags: Vec<Tag>,
    raws: Vec<Raw>,
}

struct Scanner<'a> {
    m: &'a str,
    b: &'a [u8],
    s: Scan,
    /// Open elements and blocks (`if`, `:each`…), the latter marked.
    stack: Vec<(&'a str, bool)>,
    line: usize,
    /// Only whitespace and end tags so far on a line that starts in text,
    /// and the shallowest depth they reached.
    leading: bool,
    min: usize,
    /// Nothing at all yet on a line that starts in text.
    fresh: bool,
}

impl Scan {
    fn run(m: &str) -> Option<Scan> {
        let mut sc = Scanner {
            m,
            b: m.as_bytes(),
            s: Scan {
                plans: vec![Plan::Indent(0)],
                ..Scan::default()
            },
            stack: Vec::new(),
            line: 0,
            leading: true,
            min: 0,
            fresh: true,
        };
        sc.run()?;
        Some(sc.s)
    }
}

impl<'a> Scanner<'a> {
    fn run(&mut self) -> Option<()> {
        let b = self.b;
        let mut i = 0;
        while i < b.len() {
            match b[i] {
                b'\n' => {
                    self.newline(None);
                    i += 1;
                }
                b' ' | b'\t' => i += 1,
                b'<' if b[i..].starts_with(b"<!--") => {
                    let end = i + 4 + self.m[i + 4..].find("-->")? + 3;
                    self.token();
                    self.keep(i, end);
                    i = end;
                }
                b'<' if b.get(i + 1) == Some(&b'!') => {
                    let end = i + self.m[i..].find('>')? + 1;
                    self.token();
                    self.keep(i, end);
                    i = end;
                }
                b'<' if b.get(i + 1) == Some(&b'/') => i = self.end_tag(i)?,
                b'<' if b.get(i + 1).is_some_and(u8::is_ascii_alphabetic) => i = self.tag(i)?,
                b'{' => i = self.hole(i)?,
                _ => {
                    self.token();
                    i += 1;
                }
            }
        }
        self.stack.is_empty().then_some(())
    }

    /// A line starts: in text (`None`) or as `plan` says.
    fn newline(&mut self, plan: Option<Plan>) {
        self.line += 1;
        let depth = self.stack.len();
        self.s.plans.push(plan.unwrap_or(Plan::Indent(depth)));
        self.leading = plan.is_none();
        self.fresh = plan.is_none();
        self.min = depth;
    }

    /// The lines that start inside `from..to` stay as they are.
    fn keep(&mut self, from: usize, to: usize) {
        for _ in self.b[from..to].iter().filter(|&&c| c == b'\n') {
            self.newline(Some(Plan::Keep));
        }
    }

    /// Something other than whitespace or an end tag.
    fn token(&mut self) {
        self.leading = false;
        self.fresh = false;
    }

    /// An end tag (or a block's end, `{/if}`), or `{:else}` (`mid`), at the
    /// start of a line: the line goes out a level.
    fn dedent(&mut self, mid: bool) {
        self.fresh = false;
        if self.leading {
            let depth = self.stack.len() - usize::from(mid);
            self.min = self.min.min(depth);
            self.s.plans[self.line] = Plan::Indent(self.min);
        }
    }

    fn end_tag(&mut self, i: usize) -> Option<usize> {
        let b = self.b;
        let mut j = i + 2;
        while j < b.len() && !b[j].is_ascii_whitespace() && b[j] != b'>' {
            j += 1;
        }
        let name = &self.m[i + 2..j];
        while j < b.len() && b[j] != b'>' {
            if b[j] != b' ' && b[j] != b'\t' {
                return None;
            }
            j += 1;
        }
        let (open, block) = self.stack.pop()?;
        if block || !open.eq_ignore_ascii_case(name) {
            return None;
        }
        self.dedent(false);
        Some(j + 1)
    }

    /// `{…}` in text: a block's start, middle or end, or a hole.
    fn hole(&mut self, i: usize) -> Option<usize> {
        let e = hole_end(self.b, i + 1)?;
        let inner = self.m[i + 1..e].trim_start();
        let (client, rest) = match inner.strip_prefix(':') {
            Some(r) => (true, r),
            None => (false, inner),
        };
        let word = |s: &'a str| {
            let s = s.trim_start();
            &s[..s
                .find(|c: char| !c.is_ascii_alphanumeric())
                .unwrap_or(s.len())]
        };
        let name = |kw: &str| {
            if client {
                format!(":{kw}")
            } else {
                kw.to_string()
            }
        };
        if let Some(r) = rest.strip_prefix('#') {
            self.token();
            // A block's name, kept as text in the stack by a fixed table.
            self.stack.push((block_name(&name(word(r)))?, true));
        } else if let Some(r) = rest.strip_prefix('/') {
            let (open, block) = self.stack.pop()?;
            if !block || open != name(word(r)) {
                return None;
            }
            self.dedent(false);
        } else if client && matches!(word(rest), "else" | "case" | "then" | "catch") {
            if !self.stack.last()?.1 {
                return None;
            }
            self.dedent(true);
        } else {
            self.token();
        }
        self.keep(i, e);
        Some(e + 1)
    }

    /// A start tag at `i`; returns the byte after it (and after the
    /// content of a raw-text element).
    fn tag(&mut self, i: usize) -> Option<usize> {
        let (m, b) = (self.m, self.b);
        let depth = self.stack.len();
        let line = self.line;
        let begins_line = self.fresh;
        self.token();
        let mut j = i + 1;
        while j < b.len()
            && (b[j].is_ascii_alphanumeric() || matches!(b[j], b'-' | b':' | b'.' | b'_'))
        {
            j += 1;
        }
        let name_end = j;
        let name = &m[i + 1..j];
        let mut attrs = Vec::new();
        let mut joinable = true;
        let self_closing = loop {
            let before = j;
            while j < b.len() && b[j].is_ascii_whitespace() {
                if b[j] == b'\n' {
                    self.newline(Some(Plan::Attr(depth + 1)));
                }
                j += 1;
            }
            match b.get(j)? {
                b'>' => {
                    j += 1;
                    break false;
                }
                b'/' if b.get(j + 1) == Some(&b'>') => {
                    j += 2;
                    break true;
                }
                _ if j == before => return None,
                _ => {}
            }
            let a = j;
            if b[j] == b'{' {
                j = hole_end(b, j + 1)? + 1;
            } else {
                while j < b.len()
                    && !b[j].is_ascii_whitespace()
                    && !matches!(b[j], b'=' | b'>' | b'"' | b'\'' | b'<' | b'`' | b'{')
                    && !(b[j] == b'/' && b.get(j + 1) == Some(&b'>'))
                {
                    j += 1;
                }
                if j == a {
                    return None;
                }
                if b[j] == b'=' {
                    j = self.value(j + 1, m[a..j].contains(':'))?;
                }
            }
            if m[a..j].contains('\n') {
                joinable = false;
                self.keep(a, j);
            }
            attrs.push((a, j));
        };
        if begins_line {
            self.s.tags.push(Tag {
                line,
                last_line: self.line,
                start: i,
                end: j,
                name_end,
                attrs,
                self_closing,
                joinable,
            });
        }
        let lower = name.to_ascii_lowercase();
        if self_closing || VOID.contains(&lower.as_str()) {
            return Some(j);
        }
        if matches!(lower.as_str(), "script" | "style" | "pre" | "textarea") {
            return self.raw(i, j, &lower, depth);
        }
        self.stack.push((name, false));
        Some(j)
    }

    /// An attribute's value at `v`, right after its `=`: quoted, `{…}` or
    /// bare. A directive's (`on:click="…"`) quotes hold JavaScript, with no
    /// holes. Single quotes and none become double when nothing is lost.
    fn value(&mut self, v: usize, directive: bool) -> Option<usize> {
        let (m, b) = (self.m, self.b);
        match *b.get(v)? {
            q @ (b'"' | b'\'') => {
                let mut k = v + 1;
                while *b.get(k)? != q {
                    k = if b[k] == b'{' && !directive {
                        hole_end(b, k + 1)? + 1
                    } else {
                        k + 1
                    };
                }
                let text = &m[v + 1..k];
                if q == b'\'' && !text.contains(['"', '{']) {
                    self.s.quotes.push((v, k + 1, format!("\"{text}\"")));
                }
                Some(k + 1)
            }
            b'{' => Some(hole_end(b, v + 1)? + 1),
            _ => {
                let mut k = v;
                while k < b.len() && !b[k].is_ascii_whitespace() && b[k] != b'>' {
                    if matches!(b[k], b'"' | b'\'' | b'`' | b'=' | b'<' | b'{' | b'}') {
                        return None;
                    }
                    k += 1;
                }
                let text = &m[v..k];
                if text.is_empty() || text.ends_with('/') {
                    return None;
                }
                self.s.quotes.push((v, k, format!("\"{text}\"")));
                Some(k)
            }
        }
    }

    /// The content of a `<script>`, `<style>`, `<pre>` or `<textarea>` whose
    /// start tag is `start..j`, and its end tag.
    fn raw(&mut self, start: usize, j: usize, name: &str, depth: usize) -> Option<usize> {
        let (m, b) = (self.m, self.b);
        let close = format!("</{name}");
        let p = j
            + (b[j..].windows(close.len()))
                .position(|w| w.eq_ignore_ascii_case(close.as_bytes()))?;
        let end = p + m[p..].find('>')? + 1;
        if m[p + close.len()..end - 1].contains(|c: char| c != ' ' && c != '\t') {
            return None;
        }
        let open_line = self.line;
        self.keep(j, p);
        let opens_line = m[j..]
            .split('\n')
            .next()
            .is_some_and(|l| l.trim().is_empty());
        let line_start = m[..p].rfind('\n').map_or(0, |n| n + 1);
        let closes_alone = self.line > open_line && m[line_start..p].trim().is_empty();
        if matches!(name, "script" | "style") && closes_alone {
            self.s.plans[self.line] = Plan::Indent(depth);
            let typed = m[start..j].contains("type=");
            if opens_line && (name == "style" || !typed) {
                self.s.raws.push(Raw {
                    first: open_line + 1,
                    close: self.line,
                    depth,
                    style: name == "style",
                });
            }
        }
        self.token();
        Some(end)
    }
}

/// The names blocks are kept by, so the stack holds `&str`s of the input
/// and these.
fn block_name(kw: &str) -> Option<&'static str> {
    const NAMES: [&str; 14] = [
        "if", "each", "match", "snippet", "key", "await", "try", ":if", ":each", ":match",
        ":snippet", ":key", ":await", ":try",
    ];
    NAMES.into_iter().find(|n| *n == kw)
}

/// `m` with each range in `edits` (in order, apart) replaced.
fn requote(m: &str, edits: &[(usize, usize, String)]) -> String {
    let mut out = String::with_capacity(m.len() + 2 * edits.len());
    let mut at = 0;
    for (from, to, text) in edits {
        out.push_str(&m[at..*from]);
        out.push_str(text);
        at = *to;
    }
    out.push_str(&m[at..]);
    out
}

fn pad(out: &mut String, level: usize) {
    for _ in 0..level {
        out.push_str("  ");
    }
}

/// The markup laid out as `s` planned it.
fn render(m: &str, s: &Scan) -> Option<String> {
    let lines: Vec<&str> = m.split('\n').collect();
    if lines.len() != s.plans.len() {
        return None;
    }
    // Trailing whitespace goes where the line's end is between tokens.
    let soft_end = |k: usize| {
        s.plans.get(k + 1).is_none_or(|p| *p != Plan::Keep)
            || s.raws.iter().any(|r| r.first == k + 1)
    };
    let starts: Vec<usize> = (lines.iter())
        .scan(0, |at, l| {
            let start = *at;
            *at += l.len() + 1;
            Some(start)
        })
        .collect();
    let mut out: Vec<(String, bool)> = Vec::with_capacity(lines.len());
    let mut tags = s.tags.iter().filter(|t| t.joinable).peekable();
    let mut raws = s.raws.iter().peekable();
    let mut k = 0;
    while k < lines.len() {
        while tags.peek().is_some_and(|t| t.line < k) {
            tags.next();
        }
        // The line's last: a start tag's lines are one.
        let mut last = k;
        if let (Some(t), Plan::Indent(n)) = (tags.next_if(|t| t.line == k), s.plans[k]) {
            last = t.last_line;
            let end_line = &m[t.end..starts[last] + lines[last].len()];
            let rest = if soft_end(last) {
                end_line.trim_end()
            } else {
                end_line
            };
            out.push((layout_tag(m, t, n, rest), false));
        } else {
            let l = lines[k];
            let n = match s.plans[k] {
                Plan::Keep => None,
                Plan::Indent(n) => Some(n),
                Plan::Attr(n) if l.trim_start().starts_with(['>', '/']) => {
                    Some(n.saturating_sub(1))
                }
                Plan::Attr(n) => Some(n),
            };
            match n {
                None => out.push((l.to_string(), false)),
                Some(n) => {
                    let text = l.trim_start();
                    let text = if soft_end(k) { text.trim_end() } else { text };
                    let mut line = String::new();
                    if !text.is_empty() {
                        pad(&mut line, n);
                        line.push_str(text);
                    }
                    let blank = line.is_empty();
                    out.push((line, blank));
                }
            }
        }
        k = last + 1;
        if let Some(r) = raws.next_if(|r| r.first == k) {
            let body = &lines[r.first..r.close];
            let done = if r.style {
                style(body, r.depth)
            } else {
                script(body, r.depth)
            };
            match done {
                Some(done) => out.extend(done.into_iter().map(|l| (l, false))),
                None => out.extend(body.iter().map(|l| (l.to_string(), false))),
            }
            k = r.close;
        }
    }
    // One blank line at most, none first or last.
    let mut text = String::with_capacity(m.len() + m.len() / 8);
    let mut was_blank = true;
    for (line, blank) in &out {
        if *blank && was_blank {
            continue;
        }
        was_blank = *blank;
        text.push_str(line);
        text.push('\n');
    }
    while text.ends_with("\n\n") {
        text.pop();
    }
    if text == "\n" {
        text.clear();
    }
    Some(text)
}

/// A start tag that begins its line, at `level`, and the rest of its last
/// line: on one line when it fits, else an attribute a line.
fn layout_tag(m: &str, t: &Tag, level: usize, rest: &str) -> String {
    let close = if t.self_closing { " />" } else { ">" };
    let name = &m[t.start..t.name_end];
    let attrs: Vec<&str> = t.attrs.iter().map(|&(a, e)| &m[a..e]).collect();
    let one = attrs.iter().map(|a| a.chars().count() + 1).sum::<usize>()
        + name.chars().count()
        + close.len();
    let mut out = String::new();
    pad(&mut out, level);
    out.push_str(name);
    if attrs.len() < 2 || 2 * level + one <= WIDTH {
        for a in &attrs {
            out.push(' ');
            out.push_str(a);
        }
    } else {
        for a in &attrs {
            out.push('\n');
            pad(&mut out, level + 1);
            out.push_str(a);
        }
    }
    out.push_str(close);
    out.push_str(rest);
    out
}

/// A script's lines indented one level past its tag, keeping their own
/// indentation. Not one with a template literal or a `\` line end, whose
/// lines may be text; nor tabs.
fn script(lines: &[&str], depth: usize) -> Option<Vec<String>> {
    let indent = |l: &str| l.len() - l.trim_start().len();
    if (lines.iter())
        .any(|l| l.contains('`') || l.trim_end().ends_with('\\') || l[..indent(l)].contains('\t'))
    {
        return None;
    }
    let min = (lines.iter())
        .filter(|l| !l.trim().is_empty())
        .map(|l| indent(l))
        .min()
        .unwrap_or(0);
    Some(
        (lines.iter())
            .map(|l| {
                let mut out = String::new();
                if !l.trim().is_empty() {
                    pad(&mut out, depth + 1);
                    out.push_str(l[min..].trim_end());
                }
                out
            })
            .collect(),
    )
}

/// A style's rules indented by nesting, a declaration a line. Only CSS
/// with no strings, escapes, comments or `url(`, where whitespace is all
/// that changes; other CSS is indented as a script is.
fn style(lines: &[&str], depth: usize) -> Option<Vec<String>> {
    let css = lines.join("\n");
    if css.contains(['"', '\'', '\\']) || css.contains("/*") || css.contains("url(") {
        return script(lines, depth);
    }
    let collapse = |s: &str| s.split_whitespace().collect::<Vec<_>>().join(" ");
    let decl = |d: &str| match d.split_once(':') {
        Some((p, v)) if p.trim().starts_with("--") => format!("{}: {}", p.trim(), v.trim()),
        Some((p, v)) => format!("{}: {}", p.trim(), collapse(v)),
        None => collapse(d),
    };
    let mut out = Vec::new();
    let mut level = depth + 1;
    let mut buf = String::new();
    let line = |level: usize, text: &str| {
        let mut l = String::new();
        pad(&mut l, level);
        l.push_str(text);
        l
    };
    for c in css.chars() {
        match c {
            '{' => {
                let sel = collapse(&buf);
                if sel.is_empty() {
                    return None;
                }
                let lead = &buf[..buf.len() - buf.trim_start().len()];
                if lead.matches('\n').count() > 1 && !out.is_empty() {
                    out.push(String::new());
                }
                out.push(line(level, &format!("{sel} {{")));
                level += 1;
                buf.clear();
            }
            ';' => {
                if buf.trim().is_empty() {
                    return None;
                }
                out.push(line(level, &format!("{};", decl(buf.trim()))));
                buf.clear();
            }
            '}' => {
                if !buf.trim().is_empty() {
                    out.push(line(level, &decl(buf.trim())));
                }
                if level == depth + 1 {
                    return None;
                }
                level -= 1;
                out.push(line(level, "}"));
                buf.clear();
            }
            c => buf.push(c),
        }
    }
    (buf.trim().is_empty() && level == depth + 1).then_some(out)
}

/// Whether two markups parse to one template, but for whitespace, quotes,
/// line numbers and the shape hash, which follows the text.
fn same(a: &str, b: &str) -> bool {
    match (template::parse(a), template::parse(b)) {
        (Ok(a), Ok(b)) => key(a) == key(b),
        _ => false,
    }
}

fn key(mut t: template::Template) -> Vec<u8> {
    t.shape = 0;
    if let Some((_, line)) = &mut t.props {
        *line = 0;
    }
    let d = format!("{t:?}");
    let b = d.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'\\' => {
                if !matches!(b.get(i + 1), Some(b'n' | b't' | b'r' | b'"' | b'\'')) {
                    out.extend_from_slice(&b[i..(i + 2).min(b.len())]);
                }
                i += 2;
                continue;
            }
            c if c.is_ascii_whitespace() || c == b'"' || c == b'\'' => {}
            _ => {
                if let Some(k) = ["line: ", "col: "]
                    .iter()
                    .find(|k| b[i..].starts_with(k.as_bytes()) && !b[i - 1].is_ascii_alphanumeric())
                {
                    i += k.len();
                    while b.get(i).is_some_and(u8::is_ascii_digit) {
                        i += 1;
                    }
                    continue;
                }
                out.push(b[i]);
            }
        }
        i += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Formats `src`, checking that once more changes nothing and that the
    /// markup parses as before.
    fn fmt(src: &str) -> String {
        let once = format(src, "2024");
        assert_eq!(format(&once, "2024"), once, "not idempotent for:\n{src}");
        let (a, _) = crate::parse_wisp(src).unwrap();
        let (b, _) = crate::parse_wisp(&once).unwrap();
        assert!(key(a) == key(b), "meaning changed:\n{src}\n---\n{once}");
        once
    }

    #[test]
    fn indents_elements_and_blocks() {
        let src = "<div>\n<p>a</p>\n    {#if x}\n<b>y</b>\n{:else if z}\n  <i>z</i>\n  {:else}\nno\n{/if}\n</div>\n";
        assert_eq!(
            fmt(src),
            "<div>\n  <p>a</p>\n  {#if x}\n    <b>y</b>\n  {:else if z}\n    <i>z</i>\n  {:else}\n    no\n  {/if}\n</div>\n"
        );
        let each = "{#each xs as x}\n<li>{x}</li>\n{:else}\n<li>none</li>\n{/each}\n{:#if open}\n<p>o</p>\n{:/if}\n";
        assert_eq!(
            fmt(each),
            "{#each xs as x}\n  <li>{x}</li>\n{:else}\n  <li>none</li>\n{/each}\n{:#if open}\n  <p>o</p>\n{:/if}\n"
        );
        // An end tag first on its line goes out with it; text keeps its words.
        assert_eq!(
            fmt("<p>\nsome  text <b>bold</b>\n  </p>\n\n\n<br>\n"),
            "<p>\n  some  text <b>bold</b>\n</p>\n\n<br>\n"
        );
    }

    #[test]
    fn keeps_raw_text() {
        let src = "<div>\n<pre>a\n   b\n  </pre>\n<textarea name=\"t\">x\n    y</textarea>\n<!-- a\n      b -->\n</div>\n";
        assert_eq!(
            fmt(src),
            "<div>\n  <pre>a\n   b\n  </pre>\n  <textarea name=\"t\">x\n    y</textarea>\n  <!-- a\n      b -->\n</div>\n"
        );
        // A hole's lines are Rust, which may be in a string.
        let hole = "<p>\n{format!(\"a\n   b\")}\n</p>\n";
        assert_eq!(fmt(hole), "<p>\n  {format!(\"a\n   b\")}\n</p>\n");
    }

    #[test]
    fn quotes_and_wraps_attributes() {
        assert_eq!(
            fmt("<a href='/x' class=big   title=\"t\" data-q='say \"hi\"'>x</a>\n"),
            "<a href=\"/x\" class=\"big\" title=\"t\" data-q='say \"hi\"'>x</a>\n"
        );
        let long = "<div>\n<button class=\"button primary\" formaction=\"?/restart\" title=\"Play again\" bind:this=\"enter\" data-x=\"1\">Go</button>\n</div>\n";
        let wrapped = "<div>\n  <button\n    class=\"button primary\"\n    formaction=\"?/restart\"\n    title=\"Play again\"\n    bind:this=\"enter\"\n    data-x=\"1\">Go</button>\n</div>\n";
        assert_eq!(fmt(long), wrapped);
        // Wrapped by hand but short: one line.
        assert_eq!(
            fmt("<input name=\"a\"\n       value={x}\n       required>\n"),
            "<input name=\"a\" value={x} required>\n"
        );
        assert_eq!(
            fmt("<svg><path d=\"M0\"/></svg>\n<path d=\"M0\"/>\n"),
            "<svg><path d=\"M0\"/></svg>\n<path d=\"M0\" />\n"
        );
    }

    #[test]
    fn scripts_and_styles() {
        let src = "<script>\n      let a = 1\n      if (a) {\n          a++\n      }   \n</script>\n<style>\n.a{color:red;margin:0  auto}\n\n@media (max-width: 600px) { .b { padding : 1px } }\n</style>\n";
        assert_eq!(
            fmt(src),
            "<script>\n  let a = 1\n  if (a) {\n      a++\n  }\n</script>\n<style>\n  .a {\n    color: red;\n    margin: 0 auto\n  }\n\n  @media (max-width: 600px) {\n    .b {\n      padding: 1px\n    }\n  }\n</style>\n"
        );
        // A template literal's lines are text: left as they are.
        let tpl = "<script>\n    let s = `a\n  b`\n</script>\n";
        assert_eq!(fmt(tpl), tpl);
        // CSS with strings is only indented.
        let css = "<style>\n.a { content: \"x;y\" }\n</style>\n";
        assert_eq!(fmt(css), "<style>\n  .a { content: \"x;y\" }\n</style>\n");
    }

    #[test]
    fn leaves_what_it_cannot_read() {
        for src in [
            "<ul>\n<li>a\n<li>b\n</ul>\n",
            "<div>\n{#if x}\n</div>\n{/if}\n",
            "<p>\n  a\r</p>\n",
            "---\nlet x = 1;\n<p>{x}</p>\n",
        ] {
            assert_eq!(format(src, "2024"), src);
        }
    }

    #[test]
    fn line_ends_and_block() {
        assert_eq!(
            fmt("<div>\r\n<p>a</p>\r\n</div>\r\n"),
            "<div>\r\n  <p>a</p>\r\n</div>\r\n"
        );
        let src = "---\nlet   x =  \"a\n  b\";\n---\n<p>{x}</p>\n";
        let out = fmt(src);
        // Without rustfmt the block stays as written.
        assert!(
            out == "---\nlet x = \"a\n  b\";\n---\n<p>{x}</p>\n" || out == src,
            "{out}"
        );
    }

    #[test]
    fn edition_from_cargo_toml() {
        let root = std::env::temp_dir().join(format!("wisp-fmt-ed-{}", std::process::id()));
        let app = root.join("app");
        std::fs::create_dir_all(app.join("src")).unwrap();
        let file = app.join("src").join("x.wisp");
        let toml = |dir: &Path, text: &str| std::fs::write(dir.join("Cargo.toml"), text).unwrap();
        toml(&root, "[workspace.package]\nedition = \"2018\"\n");
        toml(&app, "[package]\nedition.workspace = true\n");
        assert_eq!(edition(&file), "2018");
        toml(&app, "[package]\nname = \"x\"\nedition = \"2021\"\n");
        assert_eq!(edition(&file), "2021");
        let _ = std::fs::remove_dir_all(&root);
        // rustfmt sorts `use` names by edition: 2024 as `cargo fmt` does there.
        let src = "---\nuse a::{a_b, Zb, ZA};\n---\n<p></p>\n";
        let new = format(src, "2024");
        // Without rustfmt the block stays as written.
        if new != src {
            assert_eq!(new, "---\nuse a::{ZA, Zb, a_b};\n---\n<p></p>\n");
            assert_eq!(format(src, "2021"), src);
        }
    }

    /// Every example formats to itself the second time, and to the same
    /// template.
    #[test]
    fn examples() {
        fn walk(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
            for e in std::fs::read_dir(dir).unwrap().flatten() {
                let p = e.path();
                if p.is_dir() {
                    walk(&p, out);
                } else if p.extension().is_some_and(|x| x == "wisp") {
                    out.push(p);
                }
            }
        }
        let mut files = Vec::new();
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples");
        walk(&root, &mut files);
        assert!(!files.is_empty());
        for f in files {
            fmt(&std::fs::read_to_string(&f).unwrap());
        }
    }
}
