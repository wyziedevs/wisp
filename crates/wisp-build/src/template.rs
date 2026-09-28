//! The `.wisp` template language: HTML with Rust holes.
//!
//! Parsing strips comments, collapses indentation and produces a node tree
//! whose static text lives in `Template::chunks`. Every child list alternates
//! `Text, X, Text, X, …, Text` (texts may be empty). Because of that, editing
//! only static text never changes the template's *shape* (the nodes, their
//! code and their HTML context), and dev builds can hot-swap text without a
//! compile. Anything that could change what the generated Rust does is part
//! of the shape.

use crate::fnv1a;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Code {
    pub src: String,
    pub line: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Node {
    /// Index into `Template::chunks`.
    Text(usize),
    /// `{expr}`, HTML-escaped. `quote` wraps it in `"…"` (from `attr={expr}`).
    Expr { code: Code, quote: bool },
    /// `disabled={cond}`: ` disabled` when `cond` is true, nothing otherwise.
    Bool { name: String, code: Code },
    /// `{@html expr}`, not escaped.
    Html(Code),
    /// `{@const name = expr}`.
    Const(Code),
    /// `{@render children()}` in a layout.
    Render,
    If { branches: Vec<(Code, Vec<Node>)>, otherwise: Option<Vec<Node>> },
    Each { iter: Code, pat: String, index: Option<String>, body: Vec<Node>, otherwise: Option<Vec<Node>> },
    Match { scrutinee: Code, arms: Vec<(Code, Vec<Node>)> },
    /// `<wisp:head>…</wisp:head>`: output goes to the document head.
    Head(Vec<Node>),
}

#[derive(Debug)]
pub struct Template {
    pub nodes: Vec<Node>,
    pub chunks: Vec<String>,
    /// Hash of everything except static text. Equal shape ⇒ hot-swappable.
    pub shape: u64,
    pub uses_children: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error {
    pub line: u32,
    pub col: u32,
    pub msg: String,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "{}:{}: {}", self.line, self.col, self.msg)
    }
}

pub fn parse(src: &str) -> Result<Template, Error> {
    let mut p = Parser {
        src,
        b: src.as_bytes(),
        i: 0,
        ctx: Ctx::Text,
        tag: String::new(),
        tag_pos: 0,
        closing: false,
        attr: String::new(),
        last: b' ',
        preserve: 0,
        text: String::new(),
        chunks: Vec::new(),
        root: Vec::new(),
        frames: Vec::new(),
        uses_children: false,
        line_starts: std::iter::once(0).chain(src.match_indices('\n').map(|(i, _)| i + 1)).collect(),
    };
    p.run()?;

    // Trim the template as a whole; inner whitespace was already collapsed.
    if let Some(&Node::Text(first)) = p.root.first() {
        p.chunks[first] = p.chunks[first].trim_start().to_string();
    }
    if let Some(&Node::Text(last)) = p.root.last() {
        p.chunks[last] = p.chunks[last].trim_end().to_string();
    }

    let mut h = Vec::new();
    shape(&p.root, &mut h);
    Ok(Template { shape: fnv1a(&h), nodes: p.root, chunks: p.chunks, uses_children: p.uses_children })
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Ctx {
    Text,
    Tag,
    Quoted(u8),
}

enum Frame {
    If { pos: usize, branches: Vec<(Code, Vec<Node>)>, otherwise: Option<Vec<Node>> },
    Each { pos: usize, iter: Code, pat: String, index: Option<String>, body: Vec<Node>, otherwise: Option<Vec<Node>> },
    Match { pos: usize, scrutinee: Code, arms: Vec<(Code, Vec<Node>)> },
    Head { pos: usize, body: Vec<Node> },
}

struct Parser<'a> {
    src: &'a str,
    b: &'a [u8],
    i: usize,
    ctx: Ctx,
    /// Lowercased name of the tag being scanned, its position, and whether it is `</…>`.
    tag: String,
    tag_pos: usize,
    closing: bool,
    /// Lowercased name of the attribute being scanned inside a tag.
    attr: String,
    /// Last significant byte inside a tag; `=` means a value comes next.
    last: u8,
    /// Depth of `<pre>`/`<textarea>`, where whitespace is kept verbatim.
    preserve: u32,
    text: String,
    chunks: Vec<String>,
    root: Vec<Node>,
    frames: Vec<Frame>,
    uses_children: bool,
    line_starts: Vec<usize>,
}

impl Parser<'_> {
    fn run(&mut self) -> Result<(), Error> {
        while self.i < self.b.len() {
            let c = self.b[self.i];
            match self.ctx {
                Ctx::Text => match c {
                    b'{' => self.hole()?,
                    b'<' => self.tag_open()?,
                    _ if is_ws(c) => self.whitespace(),
                    _ => self.copy_until(|c| c == b'{' || c == b'<' || is_ws(c)),
                },
                Ctx::Tag => match c {
                    b'{' => self.hole()?,
                    b'>' => self.tag_close()?,
                    b'"' | b'\'' => {
                        self.push_byte(c);
                        self.ctx = Ctx::Quoted(c);
                    }
                    b'=' | b'/' => {
                        self.push_byte(c);
                        self.last = c;
                    }
                    _ if is_ws(c) => {
                        self.whitespace();
                        self.last = b' ';
                    }
                    _ => {
                        let start = self.i;
                        self.copy_until(|c| matches!(c, b'{' | b'>' | b'"' | b'\'' | b'=' | b'/') || is_ws(c));
                        if self.last != b'=' {
                            self.attr = self.src[start..self.i].to_ascii_lowercase();
                        }
                        self.last = b'a';
                    }
                },
                Ctx::Quoted(q) => match c {
                    b'{' => self.hole()?,
                    _ if c == q => {
                        self.push_byte(c);
                        self.ctx = Ctx::Tag;
                        self.last = b'a';
                    }
                    _ => self.copy_until(|c| c == b'{' || c == q),
                },
            }
        }

        if self.ctx != Ctx::Text {
            return Err(self.err(self.tag_pos, format!("unclosed <{}> tag", self.tag)));
        }
        self.flush()?;
        if let Some(f) = self.frames.last() {
            let (pos, what) = match f {
                Frame::If { pos, .. } => (*pos, "{#if}"),
                Frame::Each { pos, .. } => (*pos, "{#each}"),
                Frame::Match { pos, .. } => (*pos, "{#match}"),
                Frame::Head { pos, .. } => (*pos, "<wisp:head>"),
            };
            return Err(self.err(pos, format!("{what} is never closed")));
        }
        Ok(())
    }

    // ---- text -------------------------------------------------------------

    fn push_byte(&mut self, c: u8) {
        debug_assert!(c.is_ascii());
        self.text.push(c as char);
        self.i += 1;
    }

    /// Copies source bytes to the text buffer until `stop` matches. Stop bytes
    /// are all ASCII, so the slice boundaries are always valid UTF-8.
    fn copy_until(&mut self, stop: impl Fn(u8) -> bool) {
        let start = self.i;
        while self.i < self.b.len() && !stop(self.b[self.i]) {
            self.i += 1;
        }
        self.text.push_str(&self.src[start..self.i]);
    }

    /// A whitespace run containing a newline becomes one newline; others stay.
    /// This removes indentation without changing how the HTML renders.
    fn whitespace(&mut self) {
        let start = self.i;
        while self.i < self.b.len() && is_ws(self.b[self.i]) {
            self.i += 1;
        }
        let run = &self.src[start..self.i];
        if self.preserve == 0 && run.contains('\n') {
            self.text.push('\n');
        } else {
            self.text.push_str(run);
        }
    }

    /// Moves the text buffer into the current list as a `Text` node. Called
    /// before every structural event, which is what keeps lists alternating.
    fn flush(&mut self) -> Result<(), Error> {
        if let Some(Frame::Match { pos, arms, .. }) = self.frames.last()
            && arms.is_empty()
        {
            if !self.text.trim().is_empty() {
                return Err(self.err(*pos, "only {:case …} may follow {#match …}".into()));
            }
            self.text.clear();
            return Ok(());
        }
        let idx = self.chunks.len();
        self.chunks.push(std::mem::take(&mut self.text));
        self.list().push(Node::Text(idx));
        Ok(())
    }

    fn list(&mut self) -> &mut Vec<Node> {
        match self.frames.last_mut() {
            None => &mut self.root,
            Some(Frame::If { branches, otherwise, .. }) => match otherwise {
                Some(o) => o,
                None => &mut branches.last_mut().expect("if frame always has a branch").1,
            },
            Some(Frame::Each { body, otherwise, .. }) => match otherwise {
                Some(o) => o,
                None => body,
            },
            Some(Frame::Match { arms, .. }) => &mut arms.last_mut().expect("flush rejects nodes before the first case").1,
            Some(Frame::Head { body, .. }) => body,
        }
    }

    /// Starts a node or block at `pos`: flushes pending text into the current
    /// list. A block's own node is pushed at its close *without* another
    /// flush, since everything after its open went into the block.
    fn begin(&mut self, pos: usize) -> Result<(), Error> {
        if let Some(Frame::Match { arms, .. }) = self.frames.last()
            && arms.is_empty()
        {
            return Err(self.err(pos, "only {:case …} may follow {#match …}".into()));
        }
        self.flush()
    }

    fn push_node(&mut self, pos: usize, node: Node) -> Result<(), Error> {
        self.begin(pos)?;
        self.list().push(node);
        Ok(())
    }

    /// Closes the innermost block: flushes its last text and returns the frame.
    fn end(&mut self) -> Result<Option<Frame>, Error> {
        self.flush()?;
        Ok(self.frames.pop())
    }

    // ---- tags -------------------------------------------------------------

    fn tag_open(&mut self) -> Result<(), Error> {
        let start = self.i;
        let rest = &self.b[start + 1..];
        if rest.starts_with(b"!--") {
            // Comments are dropped, holes inside them included.
            match self.src[start + 4..].find("-->") {
                Some(n) => self.i = start + 4 + n + 3,
                None => return Err(self.err(start, "unclosed <!-- comment".into())),
            }
            return Ok(());
        }
        let closing = rest.first() == Some(&b'/');
        let name_start = start + 1 + closing as usize;
        let first = self.b.get(name_start).copied().unwrap_or(0);
        if !(first.is_ascii_alphabetic() || (!closing && first == b'!')) {
            self.push_byte(b'<'); // A literal '<' in text, like "a < b".
            return Ok(());
        }
        let mut end = name_start + 1;
        while end < self.b.len() && (self.b[end].is_ascii_alphanumeric() || matches!(self.b[end], b'-' | b':' | b'_')) {
            end += 1;
        }
        let name = self.src[name_start..end].to_ascii_lowercase();

        if name == "wisp:head" {
            let mut j = end;
            while j < self.b.len() && is_ws(self.b[j]) {
                j += 1;
            }
            if self.b.get(j) != Some(&b'>') {
                return Err(self.err(start, "<wisp:head> takes no attributes".into()));
            }
            self.i = j + 1;
            if closing {
                match self.end()? {
                    Some(Frame::Head { body, .. }) => self.list().push(Node::Head(body)),
                    _ => return Err(self.err(start, "</wisp:head> without matching <wisp:head>".into())),
                }
            } else {
                if self.frames.iter().any(|f| matches!(f, Frame::Head { .. })) {
                    return Err(self.err(start, "<wisp:head> cannot be nested".into()));
                }
                self.begin(start)?;
                self.frames.push(Frame::Head { pos: start, body: Vec::new() });
            }
            return Ok(());
        }

        self.text.push_str(&self.src[start..end]);
        self.i = end;
        self.ctx = Ctx::Tag;
        self.tag = name;
        self.tag_pos = start;
        self.closing = closing;
        self.attr.clear();
        self.last = b' ';
        if closing && matches!(self.tag.as_str(), "pre" | "textarea") {
            self.preserve = self.preserve.saturating_sub(1);
        }
        Ok(())
    }

    fn tag_close(&mut self) -> Result<(), Error> {
        self.push_byte(b'>');
        self.ctx = Ctx::Text;
        if self.closing {
            return Ok(());
        }
        match self.tag.as_str() {
            "pre" | "textarea" => self.preserve += 1,
            "script" | "style" => {
                // Raw text: no holes, no whitespace changes, until the end tag.
                let close = format!("</{}", self.tag);
                let hay = &self.src[self.i..];
                let n = hay
                    .as_bytes()
                    .windows(close.len())
                    .position(|w| w.eq_ignore_ascii_case(close.as_bytes()))
                    .ok_or_else(|| self.err(self.tag_pos, format!("unclosed <{}>", self.tag)))?;
                self.text.push_str(&hay[..n]);
                self.i += n;
            }
            _ => {}
        }
        Ok(())
    }

    // ---- holes ------------------------------------------------------------

    fn hole(&mut self) -> Result<(), Error> {
        let open = self.i;
        let end = hole_end(self.b, open + 1).ok_or_else(|| self.err(open, "unclosed {".into()))?;
        self.i = end + 1;
        let t = self.src[open + 1..end].trim();
        let line = self.line_of(open);
        let code = |s: &str| Code { src: s.trim().to_string(), line };

        // Blocks may appear anywhere, even inside tags (conditional attributes).
        if let Some(rest) = t.strip_prefix('#') {
            let (kw, arg) = split_word(rest);
            if arg.is_empty() {
                return Err(self.err(open, format!("{{#{kw}}} needs an expression")));
            }
            let frame = match kw {
                "if" => Frame::If { pos: open, branches: vec![(code(arg), Vec::new())], otherwise: None },
                "each" => {
                    let (iter, pat, index) = split_each(arg)
                        .ok_or_else(|| self.err(open, "expected {#each <expr> as <pattern>[, <index>]}".into()))?;
                    Frame::Each { pos: open, iter: code(iter), pat: pat.into(), index: index.map(Into::into), body: Vec::new(), otherwise: None }
                }
                "match" => Frame::Match { pos: open, scrutinee: code(arg), arms: Vec::new() },
                _ => return Err(self.err(open, format!("unknown block {{#{kw}}}"))),
            };
            self.begin(open)?;
            self.frames.push(frame);
            self.skip_standalone(open);
            return Ok(());
        }
        if let Some(rest) = t.strip_prefix(':').filter(|_| !t.starts_with("::")) {
            let (kw, arg) = split_word(rest);
            self.flush()?;
            match (kw, self.frames.last_mut()) {
                ("else", Some(Frame::If { branches, otherwise, .. })) if otherwise.is_none() => {
                    match split_word(arg) {
                        ("", _) => *otherwise = Some(Vec::new()),
                        ("if", c) if !c.is_empty() => branches.push((code(c), Vec::new())),
                        _ => return Err(self.err(open, "expected {:else} or {:else if <cond>}".into())),
                    }
                }
                ("else", Some(Frame::Each { otherwise, .. })) if otherwise.is_none() && arg.is_empty() => {
                    *otherwise = Some(Vec::new());
                }
                ("case", Some(Frame::Match { arms, .. })) if !arg.is_empty() => arms.push((code(arg), Vec::new())),
                _ => return Err(self.err(open, format!("{{:{kw}}} is not allowed here"))),
            }
            self.skip_standalone(open);
            return Ok(());
        }
        if let Some(rest) = t.strip_prefix('/') {
            let node = match (rest.trim(), self.end()?) {
                ("if", Some(Frame::If { branches, otherwise, .. })) => Node::If { branches, otherwise },
                ("each", Some(Frame::Each { iter, pat, index, body, otherwise, .. })) => {
                    Node::Each { iter, pat, index, body, otherwise }
                }
                ("match", Some(Frame::Match { pos, scrutinee, arms })) => {
                    if arms.is_empty() {
                        return Err(self.err(pos, "{#match} needs at least one {:case}".into()));
                    }
                    Node::Match { scrutinee, arms }
                }
                (kw, _) => return Err(self.err(open, format!("unexpected {{/{kw}}}"))),
            };
            self.list().push(node);
            self.skip_standalone(open);
            return Ok(());
        }

        // Output holes: where they may appear depends on the HTML context.
        if self.ctx == Ctx::Tag && self.last != b'=' {
            return Err(self.err(open, "inside a tag, expressions must be attribute values: name={expr}".into()));
        }
        if self.ctx != Ctx::Text && self.attr.starts_with("on") {
            return Err(self.err(open, format!("no expressions in event handler attributes like `{}`; use data-* attributes", self.attr)));
        }
        let quote = self.ctx == Ctx::Tag;
        if quote {
            self.last = b'a';
        }
        if self.ctx != Ctx::Text && BOOLEAN_ATTRS.contains(&self.attr.as_str()) {
            // On or off. A value, even "false", would turn it on.
            if !quote || t.is_empty() || t.starts_with('@') {
                return Err(self.err(open, format!("`{0}` is on or off: write {0}={{condition}}", self.attr)));
            }
            self.unwrite_attr_name(open)?;
            return self.push_node(open, Node::Bool { name: self.attr.clone(), code: code(t) });
        }

        if let Some(rest) = t.strip_prefix('@') {
            let (kw, arg) = split_word(rest);
            let node = match kw {
                "html" if self.ctx != Ctx::Text => return Err(self.err(open, "{@html} is not allowed inside tags".into())),
                "html" if !arg.is_empty() => Node::Html(code(arg)),
                "const" if arg.contains('=') => Node::Const(code(arg)),
                "render" if arg.replace(' ', "") == "children()" && self.ctx == Ctx::Text => {
                    self.uses_children = true;
                    Node::Render
                }
                _ => return Err(self.err(open, format!("unknown or malformed {{@{kw} …}}"))),
            };
            let silent = matches!(node, Node::Const(_));
            self.push_node(open, node)?;
            if silent {
                self.skip_standalone(open);
            }
            return Ok(());
        }
        if t.is_empty() {
            return Err(self.err(open, "empty {}".into()));
        }
        self.push_node(open, Node::Expr { code: code(t), quote })
    }

    /// Takes ` name=` back off the end of the text: a boolean attribute's name
    /// is printed by its node, and only when its condition holds.
    fn unwrite_attr_name(&mut self, open: usize) -> Result<(), Error> {
        let t = self.text.strip_suffix('=').unwrap_or(&self.text).trim_end();
        let at = t.len().checked_sub(self.attr.len()).filter(|&n| t.as_bytes()[n..].eq_ignore_ascii_case(self.attr.as_bytes()));
        let Some(at) = at else {
            return Err(self.err(open, format!("write {}={{condition}} in one piece", self.attr)));
        };
        let keep = t[..at].trim_end().len();
        self.text.truncate(keep);
        Ok(())
    }

    /// A block tag alone on its line (Mustache calls it standalone) leaves no
    /// line behind: the whitespace after it is dropped, so `{#each}` and
    /// `{/each}` lines do not become blank lines in the output. The newline
    /// before the tag stays, so whitespace between rendered items does too.
    fn skip_standalone(&mut self, open: usize) {
        if self.ctx != Ctx::Text || self.preserve > 0 {
            return;
        }
        let inline = |c: &u8| matches!(c, b' ' | b'\t' | b'\r');
        let line_start = self.b[..open].iter().rposition(|c| !inline(c)).is_none_or(|p| self.b[p] == b'\n');
        let rest = &self.b[self.i..];
        let line_end = rest.iter().find(|c| !inline(c)).is_none_or(|&c| c == b'\n');
        if line_start && line_end {
            while self.i < self.b.len() && is_ws(self.b[self.i]) {
                self.i += 1;
            }
        }
    }

    // ---- errors -----------------------------------------------------------

    fn line_of(&self, pos: usize) -> u32 {
        self.line_starts.partition_point(|&s| s <= pos) as u32
    }

    fn err(&self, pos: usize, msg: String) -> Error {
        let line = self.line_of(pos);
        let col = self.src[self.line_starts[line as usize - 1]..pos].chars().count() as u32 + 1;
        Error { line, col, msg }
    }
}

/// HTML's boolean attributes: present means on, whatever the value.
const BOOLEAN_ATTRS: [&str; 25] = [
    "allowfullscreen", "async", "autofocus", "autoplay", "checked", "controls", "default", "defer", "disabled",
    "formnovalidate", "hidden", "inert", "ismap", "itemscope", "loop", "multiple", "muted", "nomodule",
    "novalidate", "open", "playsinline", "readonly", "required", "reversed", "selected",
];

fn is_ws(c: u8) -> bool {
    matches!(c, b' ' | b'\t' | b'\n' | b'\r' | b'\x0c')
}

/// Splits `"kw rest"` into `("kw", "rest")`.
fn split_word(s: &str) -> (&str, &str) {
    let s = s.trim_start();
    match s.find(|c: char| c.is_whitespace()) {
        Some(n) => (&s[..n], s[n..].trim()),
        None => (s, ""),
    }
}

/// `iter as pat[, index]`: the *last* top-level `as` separates, since the
/// iterator expression may contain casts and the pattern never can.
fn split_each(arg: &str) -> Option<(&str, &str, Option<&str>)> {
    let b = arg.as_bytes();
    let mut at = None;
    for_each_top(arg, |i| {
        let word = b[i..].starts_with(b"as") && i > 0 && is_ws(b[i - 1]) && b.get(i + 2).is_some_and(|&c| is_ws(c));
        if word {
            at = Some(i);
        }
    });
    let at = at?;
    let (iter, rest) = (arg[..at].trim(), arg[at + 2..].trim());
    let mut comma = None;
    for_each_top(rest, |i| {
        if rest.as_bytes()[i] == b',' {
            comma = Some(i);
        }
    });
    let (pat, index) = match comma {
        Some(c) => (rest[..c].trim(), Some(rest[c + 1..].trim())),
        None => (rest, None),
    };
    let ident_ok = |s: &str| s.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_') && !s.is_empty();
    if iter.is_empty() || pat.is_empty() || index.is_some_and(|i| !ident_ok(i)) {
        return None;
    }
    Some((iter, pat, index))
}

/// Calls `f` with every byte index at bracket depth 0 that is outside string
/// and char literals.
pub(crate) fn for_each_top(s: &str, mut f: impl FnMut(usize)) {
    let b = s.as_bytes();
    let mut depth = 0i32;
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => depth -= 1,
            b'"' => i = skip_str(b, i),
            b'\'' => i = skip_char(b, i),
            b'r' if raw_str_start(b, i).is_some() => i = skip_raw_str(b, i),
            _ if depth == 0 => f(i),
            _ => {}
        }
        i += 1;
    }
}

/// Index of the `}` closing a hole that starts at `i`, skipping nested braces
/// and Rust string/char literals.
fn hole_end(b: &[u8], mut i: usize) -> Option<usize> {
    let mut depth = 0u32;
    while i < b.len() {
        match b[i] {
            b'{' => depth += 1,
            b'}' if depth == 0 => return Some(i),
            b'}' => depth -= 1,
            b'"' => i = skip_str(b, i),
            b'\'' => i = skip_char(b, i),
            b'r' if raw_str_start(b, i).is_some() => i = skip_raw_str(b, i),
            _ => {}
        }
        i += 1;
    }
    None
}

// The skip functions take the index of the opening byte and return the index
// of the last byte of the literal (or the end of input if unterminated).

pub(crate) fn skip_str(b: &[u8], mut i: usize) -> usize {
    i += 1;
    while i < b.len() {
        match b[i] {
            b'\\' => i += 2,
            b'"' => return i,
            _ => i += 1,
        }
    }
    b.len()
}

/// `'x'`, `'\n'`, `'\u{1F600}'` are chars; `'a` in `&'a str` is a lifetime.
pub(crate) fn skip_char(b: &[u8], i: usize) -> usize {
    match b.get(i + 1) {
        Some(b'\\') => {
            let mut j = i + 3;
            while j < b.len() && b[j] != b'\'' {
                j += 1;
            }
            j
        }
        Some(&c) => {
            let len = match c {
                0x00..=0x7f => 1,
                0xc0..=0xdf => 2,
                0xe0..=0xef => 3,
                _ => 4,
            };
            if b.get(i + 1 + len) == Some(&b'\'') { i + 1 + len } else { i }
        }
        None => i,
    }
}

/// If `b[i]` starts a raw string (`r"`, `r#"`, `br"`), returns the hash count.
pub(crate) fn raw_str_start(b: &[u8], i: usize) -> Option<usize> {
    let ident = |c: u8| c.is_ascii_alphanumeric() || c == b'_';
    let prefix_ok = i == 0 || !ident(b[i - 1]) || (b[i - 1] == b'b' && (i == 1 || !ident(b[i - 2])));
    if b[i] != b'r' || !prefix_ok {
        return None;
    }
    let mut j = i + 1;
    while j < b.len() && b[j] == b'#' {
        j += 1;
    }
    (b.get(j) == Some(&b'"')).then_some(j - i - 1)
}

pub(crate) fn skip_raw_str(b: &[u8], i: usize) -> usize {
    let hashes = raw_str_start(b, i).expect("caller checked");
    let mut j = i + 2 + hashes;
    while j < b.len() {
        if b[j] == b'"' && b[j + 1..].iter().take(hashes).filter(|&&c| c == b'#').count() == hashes {
            return j + hashes;
        }
        j += 1;
    }
    b.len()
}

/// Serializes everything but static text, for the shape hash.
fn shape(nodes: &[Node], out: &mut Vec<u8>) {
    let code = |out: &mut Vec<u8>, c: &Code| {
        out.extend_from_slice(c.src.as_bytes());
        out.push(0);
    };
    for n in nodes {
        match n {
            Node::Text(_) => out.push(b'T'),
            Node::Expr { code: c, quote } => {
                out.extend_from_slice(if *quote { b"Eq" } else { b"E" });
                code(out, c);
            }
            Node::Bool { name, code: c } => {
                out.push(b'B');
                out.extend_from_slice(name.as_bytes());
                out.push(0);
                code(out, c);
            }
            Node::Html(c) => {
                out.push(b'H');
                code(out, c);
            }
            Node::Const(c) => {
                out.push(b'C');
                code(out, c);
            }
            Node::Render => out.push(b'R'),
            Node::If { branches, otherwise } => {
                out.push(b'I');
                for (c, body) in branches {
                    code(out, c);
                    shape(body, out);
                    out.push(b';');
                }
                if let Some(o) = otherwise {
                    out.push(b'!');
                    shape(o, out);
                }
                out.push(b'.');
            }
            Node::Each { iter, pat, index, body, otherwise } => {
                out.push(b'L');
                code(out, iter);
                out.extend_from_slice(pat.as_bytes());
                out.push(0);
                out.extend_from_slice(index.as_deref().unwrap_or("").as_bytes());
                out.push(0);
                shape(body, out);
                if let Some(o) = otherwise {
                    out.push(b'!');
                    shape(o, out);
                }
                out.push(b'.');
            }
            Node::Match { scrutinee, arms } => {
                out.push(b'M');
                code(out, scrutinee);
                for (c, body) in arms {
                    code(out, c);
                    shape(body, out);
                    out.push(b';');
                }
                out.push(b'.');
            }
            Node::Head(body) => {
                out.push(b'h');
                shape(body, out);
                out.push(b'.');
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(t: &Template, n: &Node) -> String {
        match n {
            Node::Text(i) => t.chunks[*i].clone(),
            _ => panic!("not text: {n:?}"),
        }
    }

    fn assert_alternates(nodes: &[Node]) {
        assert!(nodes.len() % 2 == 1, "odd length: {nodes:?}");
        for (i, n) in nodes.iter().enumerate() {
            assert_eq!(matches!(n, Node::Text(_)), i % 2 == 0, "{nodes:?}");
            match n {
                Node::If { branches, otherwise } => {
                    branches.iter().for_each(|b| assert_alternates(&b.1));
                    otherwise.iter().for_each(|o| assert_alternates(o));
                }
                Node::Each { body, otherwise, .. } => {
                    assert_alternates(body);
                    otherwise.iter().for_each(|o| assert_alternates(o));
                }
                Node::Match { arms, .. } => arms.iter().for_each(|a| assert_alternates(&a.1)),
                Node::Head(b) => assert_alternates(b),
                _ => {}
            }
        }
    }

    #[test]
    fn plain_text() {
        let t = parse("  <p>hi</p>\n").unwrap();
        assert_eq!(t.nodes.len(), 1);
        assert_eq!(text(&t, &t.nodes[0]), "<p>hi</p>");
    }

    #[test]
    fn expr_and_whitespace() {
        let t = parse("<ul>\n    <li>{a.b}</li>\n  </ul>").unwrap();
        assert_alternates(&t.nodes);
        assert_eq!(text(&t, &t.nodes[0]), "<ul>\n<li>");
        assert!(matches!(&t.nodes[1], Node::Expr { code, quote: false } if code.src == "a.b" && code.line == 2));
        assert_eq!(text(&t, &t.nodes[2]), "</li>\n</ul>");
    }

    #[test]
    fn braces_in_rust_literals() {
        let t = parse(r#"{format!("{}}", x)}{'}'}{r"}"}"#).unwrap();
        assert_alternates(&t.nodes);
        let exprs: Vec<_> = t.nodes.iter().filter_map(|n| if let Node::Expr { code, .. } = n { Some(code.src.as_str()) } else { None }).collect();
        assert_eq!(exprs, [r#"format!("{}}", x)"#, "'}'", r#"r"}""#]);
    }

    #[test]
    fn attribute_values() {
        let t = parse(r#"<a href="/p/{id}" class={cls} data-x='{y}'>"#).unwrap();
        let quotes: Vec<_> = t.nodes.iter().filter_map(|n| if let Node::Expr { quote, .. } = n { Some(*quote) } else { None }).collect();
        assert_eq!(quotes, [false, true, false]);
        assert_eq!(text(&t, &t.nodes[2]), r#"" class="#);
    }

    #[test]
    fn rejects_unsafe_contexts() {
        assert!(parse("<div {x}>").unwrap_err().msg.contains("attribute values"));
        assert!(parse(r#"<a onclick="go({x})">"#).unwrap_err().msg.contains("event handler"));
        assert!(parse("<a onclick={x}>").unwrap_err().msg.contains("event handler"));
        assert!(parse(r#"<a title="{@html x}">"#).is_err());
    }

    #[test]
    fn script_style_comment_are_raw() {
        let t = parse("<style>a { color: red }</style><script>if (a) { b() }</script><!-- {x} -->done").unwrap();
        assert_eq!(t.nodes.len(), 1);
        assert_eq!(text(&t, &t.nodes[0]), "<style>a { color: red }</style><script>if (a) { b() }</script>done");
    }

    #[test]
    fn pre_keeps_whitespace() {
        let t = parse("<pre>\n  a\n    b</pre>\n  <p>").unwrap();
        assert_eq!(text(&t, &t.nodes[0]), "<pre>\n  a\n    b</pre>\n<p>");
    }

    #[test]
    fn standalone_block_tags_leave_no_blank_lines() {
        let t = parse("<ul>\n  {#each xs as x}\n    <li>{x}</li>\n  {/each}\n</ul>").unwrap();
        let Node::Each { body, .. } = &t.nodes[1] else { panic!("{:?}", t.nodes) };
        assert_eq!(text(&t, &t.nodes[0]), "<ul>\n");
        assert_eq!(text(&t, &body[0]), "<li>");
        assert_eq!(text(&t, &body[2]), "</li>\n");
        assert_eq!(text(&t, &t.nodes[2]), "</ul>");

        // Not alone on its line: whitespace around the tag is content.
        let t = parse("<p>{#if a}A{/if}\n<b>B</b>").unwrap();
        assert_eq!(text(&t, &t.nodes[2]), "\n<b>B</b>");
        let t = parse("<pre>\n{#if a}\nA\n{/if}\n</pre>").unwrap();
        let Node::If { branches, .. } = &t.nodes[1] else { panic!() };
        assert_eq!(text(&t, &branches[0].1[0]), "\nA\n");
    }

    #[test]
    fn blocks() {
        let src = "{#if a}\nA\n{:else if let Some(x) = b}\n{x}\n{:else}\nC\n{/if}\
                   {#each items as (k, v), i}{k}{:else}none{/each}\
                   {#match m}\n  {:case Some(x) if x > 1}big{:case _}small{/match}";
        let t = parse(src).unwrap();
        assert_alternates(&t.nodes);
        match &t.nodes[1] {
            Node::If { branches, otherwise } => {
                assert_eq!(branches.len(), 2);
                assert_eq!(branches[1].0.src, "let Some(x) = b");
                assert!(otherwise.is_some());
            }
            n => panic!("{n:?}"),
        }
        match &t.nodes[3] {
            Node::Each { iter, pat, index, otherwise, .. } => {
                assert_eq!((iter.src.as_str(), pat.as_str(), index.as_deref()), ("items", "(k, v)", Some("i")));
                assert!(otherwise.is_some());
            }
            n => panic!("{n:?}"),
        }
        match &t.nodes[5] {
            Node::Match { arms, .. } => assert_eq!(arms[0].0.src, "Some(x) if x > 1"),
            n => panic!("{n:?}"),
        }
    }

    #[test]
    fn each_with_cast() {
        assert_eq!(split_each("0..n as usize as i"), Some(("0..n as usize", "i", None)));
        assert_eq!(split_each("xs"), None);
    }

    #[test]
    fn conditional_attribute_inside_tag() {
        let t = parse("<input {#if on}checked{/if}>").unwrap();
        assert_alternates(&t.nodes);
    }

    #[test]
    fn boolean_attributes() {
        let t = parse("<button class=\"k\"
  DISABLED ={!ok} name=k>").unwrap();
        assert_alternates(&t.nodes);
        assert_eq!(text(&t, &t.nodes[0]), "<button class=\"k\"");
        assert!(matches!(&t.nodes[1], Node::Bool { name, code } if name == "disabled" && code.src == "!ok"));
        assert_eq!(text(&t, &t.nodes[2]), " name=k>");
        assert!(parse("<input checked=\"{on}\">").unwrap_err().msg.contains("on or off"));
        assert!(parse("<details open={@html x}>").is_err());
        // Anything else is a value, as before.
        assert!(matches!(&parse("<a aria-hidden={h}>").unwrap().nodes[1], Node::Expr { quote: true, .. }));
    }

    #[test]
    fn head_and_render() {
        let t = parse("<wisp:head><title>{t}</title></wisp:head><main>{@render children()}</main>").unwrap();
        assert!(t.uses_children);
        assert_alternates(&t.nodes);
        assert!(matches!(&t.nodes[1], Node::Head(b) if b.len() == 3));
    }

    #[test]
    fn errors_have_positions() {
        let e = parse("<p>\n  {#if x}\n</p>").unwrap_err();
        assert_eq!((e.line, e.col), (2, 3));
        let e = parse("{/each}").unwrap_err();
        assert!(e.msg.contains("unexpected"));
        let e = parse("{#match x} junk {:case _}{/match}").unwrap_err();
        assert!(e.msg.contains("case"));
    }

    #[test]
    fn shape_ignores_text_only() {
        let a = parse("<h1>Hello {name}</h1>").unwrap();
        let b = parse("<h2 class='big'>Goodbye {name}!</h2>").unwrap();
        let c = parse("<h1>Hello {name.len()}</h1>").unwrap();
        let d = parse("<h1 title={name}>Hello</h1>").unwrap();
        assert_eq!(a.shape, b.shape);
        assert_ne!(a.shape, c.shape);
        assert_ne!(a.shape, d.shape); // same code, different context
        assert_eq!(a.chunks.len(), b.chunks.len());
    }
}
