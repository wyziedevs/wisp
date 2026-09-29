//! Just enough JavaScript for the compiler to wire a template's browser code
//! to its server values: a tokenizer, and what the code generator asks of a
//! script or a directive's value (the names it reads, the names it declares,
//! its imports). Not a parser: nothing here needs a syntax tree, and code it
//! misreads fails in the browser, where the console points at the line.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Ident,
    Number,
    String,
    /// A template literal, or one piece of one: `` `a${ ``, `}b${`, `` }c` ``.
    Template,
    Regex,
    Punct,
}

#[derive(Debug, Clone, Copy)]
pub struct Token {
    pub kind: Kind,
    pub start: usize,
    pub end: usize,
    /// Brackets (and `${` of template literals) around the token.
    pub depth: u32,
    /// The innermost of them: `{`, `(`, `[`, `$` (a `${…}`), or 0 at the top.
    pub inner: u8,
    /// A line break comes before it (for where a statement may end).
    pub newline: bool,
    /// An identifier after `.` or `?.`: a property, not a variable.
    pub member: bool,
    /// An identifier that is an object literal's key: `{ key: value }`.
    pub key: bool,
}

impl Token {
    pub fn text<'a>(&self, src: &'a str) -> &'a str {
        &src[self.start..self.end]
    }

    fn is(&self, src: &str, kind: Kind, text: &str) -> bool {
        self.kind == kind && self.text(src) == text
    }
}

/// Reserved words: never a variable's name.
const RESERVED: [&str; 38] = [
    "await",
    "break",
    "case",
    "catch",
    "class",
    "const",
    "continue",
    "debugger",
    "default",
    "delete",
    "do",
    "else",
    "enum",
    "export",
    "extends",
    "false",
    "finally",
    "for",
    "function",
    "if",
    "import",
    "in",
    "instanceof",
    "let",
    "new",
    "null",
    "return",
    "static",
    "super",
    "switch",
    "this",
    "throw",
    "true",
    "try",
    "typeof",
    "var",
    "void",
    "while",
];

pub fn is_reserved(word: &str) -> bool {
    RESERVED.contains(&word) || matches!(word, "with" | "yield")
}

/// After these words an expression starts, so `/` there begins a regex.
const BEFORE_EXPRESSION: [&str; 14] = [
    "return",
    "typeof",
    "instanceof",
    "in",
    "of",
    "new",
    "delete",
    "void",
    "throw",
    "case",
    "do",
    "else",
    "yield",
    "await",
];

/// Longest first, so `>>>=` is not read as `>>` and `>=`.
const PUNCTS: [&str; 34] = [
    ">>>=", "...", "===", "!==", "**=", "<<=", ">>=", ">>>", "&&=", "||=", "??=", "=>", "==", "!=",
    "<=", ">=", "&&", "||", "??", "?.", "++", "--", "+=", "-=", "*=", "/=", "%=", "&=", "|=", "^=",
    "**", "<<", ">>", "#",
];

fn ident_start(c: u8) -> bool {
    c.is_ascii_alphabetic() || c == b'_' || c == b'$' || c >= 0x80
}

fn ident_byte(c: u8) -> bool {
    ident_start(c) || c.is_ascii_digit()
}

/// The tokens of `src`, comments and whitespace dropped. Never fails: what
/// it cannot read (an unclosed string) ends where the input does.
pub fn tokens(src: &str) -> Vec<Token> {
    let b = src.as_bytes();
    let mut out: Vec<Token> = Vec::new();
    // Open brackets: `{ ( [`, and `$` for a template literal's `${`.
    let mut stack: Vec<u8> = Vec::new();
    let mut newline = false;
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        if c == b'\n' {
            newline = true;
            i += 1;
            continue;
        }
        if c.is_ascii_whitespace() {
            i += 1;
            continue;
        }
        if c == b'/' && b.get(i + 1) == Some(&b'/') {
            while i < b.len() && b[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        if c == b'/' && b.get(i + 1) == Some(&b'*') {
            let end = src[i + 2..].find("*/").map_or(b.len(), |n| i + 2 + n + 2);
            newline |= src[i..end].contains('\n');
            i = end;
            continue;
        }

        let start = i;
        let depth = stack.len() as u32;
        let inner = stack.last().copied().unwrap_or(0);
        let kind = if ident_start(c) || (c == b'#' && b.get(i + 1).is_some_and(|&n| ident_start(n)))
        {
            i += 1;
            while i < b.len() && ident_byte(b[i]) {
                i += 1;
            }
            Kind::Ident
        } else if c.is_ascii_digit() || (c == b'.' && b.get(i + 1).is_some_and(u8::is_ascii_digit))
        {
            let hex = c == b'0' && matches!(b.get(i + 1), Some(b'x' | b'X'));
            i += 1;
            while i < b.len() {
                let n = b[i];
                let exponent_sign =
                    matches!(n, b'+' | b'-') && !hex && matches!(b[i - 1], b'e' | b'E');
                if !(n.is_ascii_alphanumeric() || n == b'_' || n == b'.' || exponent_sign) {
                    break;
                }
                i += 1;
            }
            Kind::Number
        } else if c == b'"' || c == b'\'' {
            i += 1;
            while i < b.len() && b[i] != c && b[i] != b'\n' {
                i += if b[i] == b'\\' { 2 } else { 1 };
            }
            i = (i + 1).min(b.len());
            Kind::String
        } else if c == b'`' {
            i = template_rest(b, i + 1, &mut stack);
            Kind::Template
        } else if c == b'}' && stack.last() == Some(&b'$') {
            // The end of a `${…}`: the literal goes on, at its own depth.
            stack.pop();
            let (depth, inner) = (stack.len() as u32, stack.last().copied().unwrap_or(0));
            i = template_rest(b, i + 1, &mut stack);
            out.push(Token {
                kind: Kind::Template,
                start,
                end: i,
                depth,
                inner,
                newline,
                member: false,
                key: false,
            });
            newline = false;
            continue;
        } else if c == b'/' && regex_allowed(src, &out) {
            i += 1;
            let mut class = false;
            while i < b.len() && b[i] != b'\n' {
                match b[i] {
                    b'\\' => i += 1,
                    b'[' => class = true,
                    b']' => class = false,
                    b'/' if !class => break,
                    _ => {}
                }
                i += 1;
            }
            i = (i + 1).min(b.len());
            while i < b.len() && ident_byte(b[i]) {
                i += 1;
            }
            Kind::Regex
        } else {
            let rest = &src[i..];
            let n = PUNCTS
                .iter()
                .find(|p| {
                    rest.starts_with(**p)
                        && !(**p == "?." && rest.as_bytes().get(2).is_some_and(u8::is_ascii_digit))
                })
                .map_or(1, |p| p.len());
            // A multi-byte character that is not an identifier's: one char.
            let n = if c >= 0x80 {
                rest.chars().next().map_or(1, char::len_utf8)
            } else {
                n
            };
            i += n;
            match c {
                b'{' | b'(' | b'[' if n == 1 => stack.push(c),
                b'}' | b')' | b']' => {
                    stack.pop();
                }
                _ => {}
            }
            Kind::Punct
        };
        // Closing brackets sit at the depth of what they close.
        let (depth, inner) = if kind == Kind::Punct && matches!(c, b'}' | b')' | b']') {
            (stack.len() as u32, stack.last().copied().unwrap_or(0))
        } else {
            (depth, inner)
        };
        out.push(Token {
            kind,
            start,
            end: i,
            depth,
            inner,
            newline,
            member: false,
            key: false,
        });
        newline = false;
    }

    for k in 0..out.len() {
        if out[k].kind != Kind::Ident {
            continue;
        }
        let prev = k.checked_sub(1).map(|p| out[p].text(src));
        out[k].member = matches!(prev, Some("." | "?."));
        let next = out.get(k + 1).map(|n| n.text(src));
        out[k].key = !out[k].member
            && matches!(prev, Some("{" | ","))
            && next == Some(":")
            && out[k].inner == b'{';
    }
    out
}

/// Scans a template literal from just after its `` ` `` or `}`: to its end
/// (after the closing `` ` ``), or to just after a `${`, which it pushes.
fn template_rest(b: &[u8], mut i: usize, stack: &mut Vec<u8>) -> usize {
    while i < b.len() {
        match b[i] {
            b'\\' => i += 2,
            b'`' => return i + 1,
            b'$' if b.get(i + 1) == Some(&b'{') => {
                stack.push(b'$');
                return i + 2;
            }
            _ => i += 1,
        }
    }
    b.len()
}

/// Whether a `/` after the tokens so far starts a regex rather than dividing.
fn regex_allowed(src: &str, before: &[Token]) -> bool {
    let Some(p) = before.last() else { return true };
    let text = p.text(src);
    match p.kind {
        Kind::Ident => {
            let property =
                before.len() > 1 && matches!(before[before.len() - 2].text(src), "." | "?.");
            !property && BEFORE_EXPRESSION.contains(&text)
        }
        Kind::Number | Kind::String | Kind::Regex => false,
        Kind::Template => text.ends_with("${"),
        Kind::Punct => !matches!(text, ")" | "]" | "++" | "--"),
    }
}

/// The token closing the bracket at `k` (closers sit at their opener's
/// depth), or the end.
fn close(t: &[Token], k: usize) -> usize {
    (k + 1..t.len())
        .find(|&j| t[j].depth <= t[k].depth)
        .unwrap_or(t.len())
}

/// The names a binding pattern at `k` binds (a name, or `{…}`, `[…]` or a
/// parameter list `(…)`), as token indexes, and the index after it.
/// Default values and object keys are not names.
fn pattern(src: &str, t: &[Token], k: usize, out: &mut Vec<usize>) -> usize {
    let Some(tok) = t.get(k) else { return k };
    if tok.kind == Kind::Ident {
        if !is_reserved(tok.text(src)) {
            out.push(k);
        }
        return k + 1;
    }
    if !matches!(tok.text(src), "{" | "[" | "(") || tok.kind != Kind::Punct {
        return k;
    }
    let end = close(t, k);
    let mut j = k + 1;
    while j < end {
        let n = t[j];
        if n.is(src, Kind::Punct, "=") {
            // A default: skip to the next item at this level.
            let d = n.depth;
            while j < end && !(t[j].depth == d && t[j].is(src, Kind::Punct, ",")) {
                j += 1;
            }
            continue;
        }
        if n.kind == Kind::Ident && !n.member && !n.key && !is_reserved(n.text(src)) {
            out.push(j);
        }
        j += 1;
    }
    end + 1
}

/// Per token: a name bound inside `src` rather than read from outside it:
/// a function's (or arrow's, or `catch`'s) parameter within that function,
/// or a `let`, `const`, `var`, `class` or `function` inside a block, within
/// that block. Top-level names are `declarations`'.
fn bound(src: &str, t: &[Token]) -> Vec<bool> {
    // (name token indexes, first and last token they are visible in)
    let mut scopes: Vec<(Vec<usize>, usize, usize)> = Vec::new();
    let is = |j: usize, s: &str| {
        t.get(j)
            .is_some_and(|n| n.kind == Kind::Punct && n.text(src) == s)
    };
    // Where the block around token `k` ends.
    let block_end = |k: usize| {
        (k + 1..t.len())
            .find(|&j| t[j].depth < t[k].depth)
            .unwrap_or(t.len())
    };
    // A parameter list at `p` and the `{…}` body after it.
    let function = |p: usize, scopes: &mut Vec<(Vec<usize>, usize, usize)>| {
        let mut names = Vec::new();
        let body = pattern(src, t, p, &mut names);
        if is(body, "{") {
            scopes.push((names, p, close(t, body)));
        }
    };
    for k in 0..t.len() {
        let tok = t[k];
        if tok.kind == Kind::Punct && tok.text(src) == "=>" {
            // `x =>` or `(x, { y }) =>`; the body is a block or runs to the
            // end of the expression.
            let mut names = Vec::new();
            let start = match k.checked_sub(1).map(|p| t[p]) {
                Some(p) if p.kind == Kind::Ident => k - 1,
                Some(p) if p.is(src, Kind::Punct, ")") => (0..k - 1)
                    .rev()
                    .find(|&j| t[j].depth == p.depth)
                    .unwrap_or(0),
                _ => continue,
            };
            pattern(src, t, start, &mut names);
            let end = if is(k + 1, "{") {
                close(t, k + 1)
            } else {
                (k + 1..t.len())
                    .find(|&j| {
                        t[j].depth < tok.depth
                            || (t[j].depth == tok.depth && matches!(t[j].text(src), "," | ";"))
                    })
                    .unwrap_or(t.len())
            };
            scopes.push((names, start, end));
            continue;
        }
        if tok.kind != Kind::Ident || tok.member {
            continue;
        }
        let word = tok.text(src);
        match word {
            "function" => {
                let mut j = k + 1 + is(k + 1, "*") as usize;
                if t.get(j).is_some_and(|n| n.kind == Kind::Ident) {
                    if tok.depth > 0 {
                        scopes.push((vec![j], k, block_end(k)));
                    }
                    j += 1;
                }
                function(j, &mut scopes);
            }
            "catch" => function(k + 1, &mut scopes),
            "class" if tok.depth > 0 && t.get(k + 1).is_some_and(|n| n.kind == Kind::Ident) => {
                scopes.push((vec![k + 1], k, block_end(k)));
            }
            "let" | "const" | "var" if tok.depth > 0 => {
                let mut names = Vec::new();
                let mut j = pattern(src, t, k + 1, &mut names);
                // More declarators after commas; an initializer is skipped.
                loop {
                    while j < t.len()
                        && t[j].depth >= tok.depth
                        && !(t[j].depth == tok.depth && matches!(t[j].text(src), "," | ";"))
                    {
                        j += 1;
                    }
                    if !is(j, ",") || t[j].depth != tok.depth {
                        break;
                    }
                    j = pattern(src, t, j + 1, &mut names);
                }
                // `for (let x of xs) …`: the loop's body sees it too.
                let mut end = block_end(k);
                if tok.inner == b'(' && end < t.len() {
                    end = if is(end + 1, "{") {
                        close(t, end + 1)
                    } else {
                        block_end(end + 1)
                    };
                }
                scopes.push((names, k, end));
            }
            // `name(a, b) { … }`: a method, whose name is no variable.
            _ if !is_reserved(word) && is(k + 1, "(") && is(close(t, k + 1) + 1, "{") => {
                scopes.push((vec![k], k, k));
                function(k + 1, &mut scopes)
            }
            _ => {}
        }
    }
    let mut out = vec![false; t.len()];
    for (names, from, to) in scopes {
        for n in names {
            let name = t[n].text(src);
            for j in from..to.min(t.len() - 1) + 1 {
                if t[j].kind == Kind::Ident && !t[j].member && !t[j].key && t[j].text(src) == name {
                    out[j] = true;
                }
            }
        }
    }
    out
}

/// The variables `src` reads, each with the properties read from it:
/// `data.user.name` is `["data", "user", "name"]`. A chain stops before a
/// call or an index (`key.mark.class()` is `["key", "mark"]`), and at `?.`.
/// Each comes with the offset where it starts. Properties, object keys,
/// reserved words and names `src` binds itself (parameters, and
/// declarations inside blocks) are not variables read.
pub fn chains(src: &str) -> Vec<(Vec<String>, usize)> {
    let t = tokens(src);
    let own = bound(src, &t);
    let mut out = Vec::new();
    let mut k = 0;
    while k < t.len() {
        let tok = t[k];
        let word = tok.text(src);
        if tok.kind != Kind::Ident
            || tok.member
            || tok.key
            || own[k]
            || (word == "of"
                && tok.inner == b'('
                && k > 0
                && (t[k - 1].kind == Kind::Ident || matches!(t[k - 1].text(src), "]" | "}")))
            || word.starts_with('#')
            || is_reserved(word)
        {
            k += 1;
            continue;
        }
        let mut path = vec![word.to_string()];
        let mut j = k + 1;
        while j + 1 < t.len()
            && t[j].is(src, Kind::Punct, ".")
            && t[j + 1].kind == Kind::Ident
            && !t[j + 1].text(src).starts_with('#')
        {
            path.push(t[j + 1].text(src).to_string());
            j += 2;
        }
        // `a.b.c()` calls c: the value read is `a.b`.
        if path.len() > 1 && t.get(j).is_some_and(|n| n.is(src, Kind::Punct, "(")) {
            path.pop();
        }
        out.push((path, tok.start));
        k = j;
    }
    out
}

/// The names a script declares at its top level: `let`, `const` and `var`
/// (several in one statement too), `function` and `class`, each with the
/// offset of its name. Destructured names (`const { a } = x`) are not found.
pub fn declarations(src: &str) -> Vec<(String, usize)> {
    let t = tokens(src);
    let mut out = Vec::new();
    let ident = |k: usize| {
        t.get(k)
            .filter(|n| n.kind == Kind::Ident && !is_reserved(n.text(src)))
    };
    let statement_start = |k: usize| {
        k == 0
            || t[k].newline
            || matches!(
                t[k - 1].text(src),
                ";" | "}" | "async" | "export" | "default"
            )
    };
    let mut k = 0;
    while k < t.len() {
        let tok = t[k];
        if tok.depth != 0 || tok.kind != Kind::Ident || tok.member {
            k += 1;
            continue;
        }
        match tok.text(src) {
            "let" | "const" | "var" => {
                let mut names = Vec::new();
                let add = |names: &[usize], out: &mut Vec<(String, usize)>| {
                    out.extend(
                        names
                            .iter()
                            .map(|&n| (t[n].text(src).to_string(), t[n].start)),
                    );
                };
                pattern(src, &t, k + 1, &mut names);
                if names.is_empty() {
                    k += 1;
                    continue;
                }
                add(&names, &mut out);
                // More after top-level commas, until the statement ends: at a
                // `;`, or at a line break after something that ends a value.
                let mut j = k + 2;
                while j < t.len() {
                    let n = t[j];
                    if n.depth == 0 {
                        let text = n.text(src);
                        if text == ";" {
                            break;
                        }
                        let prev = t[j - 1];
                        let prev_ends_value = prev.depth == 0
                            && (matches!(
                                prev.kind,
                                Kind::Ident
                                    | Kind::Number
                                    | Kind::String
                                    | Kind::Regex
                                    | Kind::Template
                            ) || matches!(prev.text(src), ")" | "]" | "}"));
                        if n.newline
                            && prev_ends_value
                            && text != ","
                            && !matches!(n.kind, Kind::Punct)
                        {
                            break;
                        }
                        if text == "," {
                            names.clear();
                            pattern(src, &t, j + 1, &mut names);
                            add(&names, &mut out);
                        }
                    }
                    j += 1;
                }
                k = j;
            }
            "function" | "class" if statement_start(k) => {
                let mut j = k + 1;
                if t.get(j).is_some_and(|n| n.is(src, Kind::Punct, "*")) {
                    j += 1;
                }
                if let Some(name) = ident(j) {
                    out.push((name.text(src).to_string(), name.start));
                }
                k = j;
            }
            _ => k += 1,
        }
    }
    out
}

/// What the top-level `let`, `const` or `var` that declares `name` first
/// sets it to: `0` for `let n = 0`. `None` when it is not set there.
pub fn initializer<'a>(src: &'a str, name: &str) -> Option<&'a str> {
    let t = tokens(src);
    let k = (1..t.len()).find(|&k| {
        t[k].depth == 0
            && t[k].is(src, Kind::Ident, name)
            && matches!(t[k - 1].text(src), "let" | "const" | "var")
    })?;
    if !t.get(k + 1)?.is(src, Kind::Punct, "=") {
        return None;
    }
    let start = k + 2;
    // To a top-level `,` or `;`, or a line that starts another statement.
    let end = (start + 1..t.len())
        .find(|&j| {
            let prev = t[j - 1];
            t[j].depth == 0
                && (matches!(t[j].text(src), "," | ";")
                    || (t[j].newline
                        && t[j].kind != Kind::Punct
                        && prev.depth == 0
                        && (prev.kind != Kind::Punct || matches!(prev.text(src), ")" | "]" | "}"))))
        })
        .unwrap_or(t.len());
    (start < end).then(|| &src[t[start].start..t[end - 1].end])
}

/// The top-level `import … from '…'` and `import '…'` statements of a
/// script, as byte ranges, with their `with { … }` and `;` if any. Dynamic
/// `import(…)` and `import.meta` are not statements.
pub fn imports(src: &str) -> Vec<(usize, usize)> {
    let t = tokens(src);
    let mut out = Vec::new();
    let mut k = 0;
    while k < t.len() {
        let tok = t[k];
        let next = t.get(k + 1).map(|n| n.text(src));
        if tok.depth != 0
            || tok.member
            || !tok.is(src, Kind::Ident, "import")
            || matches!(next, Some("(" | ".") | None)
        {
            k += 1;
            continue;
        }
        // The module's name: right after `import`, or after `from`.
        let mut j = k + 1;
        let spec = loop {
            let Some(n) = t.get(j) else { break None };
            if n.kind == Kind::String
                && (j == k + 1 || (n.depth == 0 && t[j - 1].is(src, Kind::Ident, "from")))
            {
                break Some(j);
            }
            if n.depth == 0 && n.is(src, Kind::Punct, ";") {
                break None;
            }
            j += 1;
        };
        let Some(mut last) = spec else {
            k += 1;
            continue;
        };
        if t.get(last + 1).is_some_and(|n| {
            n.depth == 0 && (n.is(src, Kind::Ident, "with") || n.is(src, Kind::Ident, "assert"))
        }) && t.get(last + 2).is_some_and(|n| n.is(src, Kind::Punct, "{"))
        {
            let close =
                (last + 3..t.len()).find(|&m| t[m].depth == 0 && t[m].is(src, Kind::Punct, "}"));
            last = close.unwrap_or(t.len() - 1);
        }
        if t.get(last + 1).is_some_and(|n| n.is(src, Kind::Punct, ";")) {
            last += 1;
        }
        out.push((tok.start, t[last].end));
        k = last + 1;
    }
    out
}

/// `src` with each `let name = $derived(expression)` (or `const`) at its top
/// level rewritten for live.js, as `let name = __wisp_d(() => (expression),
/// (__v) => name = __v)`, which works `name` out again before every redraw.
/// Line breaks stay where they were. Any other `$derived` is an error, with
/// its offset.
pub fn derived(src: &str) -> Result<String, (usize, String)> {
    let t = tokens(src);
    let mut out = String::with_capacity(src.len());
    let mut at = 0;
    let mut k = 0;
    while k < t.len() {
        if t[k].member || !t[k].is(src, Kind::Ident, "$derived") {
            k += 1;
            continue;
        }
        let declared = k >= 3
            && t[k - 3].depth == 0
            && matches!(t[k - 3].text(src), "let" | "const")
            && t[k - 2].kind == Kind::Ident
            && t[k - 1].is(src, Kind::Punct, "=");
        let open = k + 1;
        let end = close(&t, open);
        if !declared || !t.get(open).is_some_and(|n| n.is(src, Kind::Punct, "(")) {
            return Err((
                t[k].start,
                "`$derived` declares a value at the top of the script: `let name = $derived(expression)`".into(),
            ));
        }
        if end >= t.len() || end == open + 1 {
            return Err((
                t[k].start,
                "`$derived(…)` takes the expression the value is worked out from".into(),
            ));
        }
        let (keyword, name) = (t[k - 3], t[k - 2].text(src));
        out.push_str(&src[at..keyword.start]);
        // `const` too: the value is set again on every redraw.
        out.push_str("let");
        out.push_str(&src[keyword.end..t[k].start]);
        out.push_str("__wisp_d(() => (");
        out.push_str(&src[t[open].end..t[end].start]);
        out.push_str(&format!("), (__v) => {name} = __v)"));
        at = t[end].end;
        k = end + 1;
    }
    out.push_str(&src[at..]);
    Ok(out)
}

/// `src` with each range replaced by spaces, its line breaks kept, so the
/// rest keeps its line numbers.
pub fn blank(src: &str, ranges: &[(usize, usize)]) -> String {
    let mut out = String::with_capacity(src.len());
    let mut at = 0;
    for &(start, end) in ranges {
        out.push_str(&src[at..start]);
        out.extend(
            src[start..end]
                .chars()
                .map(|c| if c == '\n' { '\n' } else { ' ' }),
        );
        at = end;
    }
    out.push_str(&src[at..]);
    out
}

/// A name or a property path, such as `toggle` or `menu.close`: what an
/// event handler calls with the event.
pub fn is_path(src: &str) -> bool {
    let t = tokens(src);
    !t.is_empty()
        && t.iter().enumerate().all(|(k, tok)| {
            if k % 2 == 0 {
                tok.kind == Kind::Ident
                    && !is_reserved(tok.text(src))
                    && !tok.text(src).starts_with('#')
            } else {
                tok.is(src, Kind::Punct, ".")
            }
        })
        && t.len() % 2 == 1
}

/// Statements rather than one expression: a top-level `;`, or a statement
/// keyword first.
pub fn is_statements(src: &str) -> bool {
    let t = tokens(src);
    let first = t.first().map(|f| f.text(src));
    matches!(
        first,
        Some(
            "if" | "for"
                | "while"
                | "let"
                | "const"
                | "var"
                | "return"
                | "throw"
                | "try"
                | "switch"
                | "do"
        )
    ) || t
        .iter()
        .any(|tok| tok.depth == 0 && tok.is(src, Kind::Punct, ";"))
}

/// Whether `src` ends in a `//` comment, so that code written after it on
/// the same line would be commented out too.
pub fn ends_in_line_comment(src: &str) -> bool {
    let end = tokens(src).last().map_or(0, |t| t.end);
    src[end..].contains("//")
}

/// `item in list` or `item, i in list`: the names, and where the list starts.
pub fn each(src: &str) -> Option<(String, Option<String>, usize)> {
    let t = tokens(src);
    let name = |k: usize| {
        t.get(k)
            .filter(|n| {
                n.kind == Kind::Ident && !is_reserved(n.text(src)) && !n.text(src).starts_with('#')
            })
            .map(|n| n.text(src).to_string())
    };
    let item = name(0)?;
    let (index, at) = if t.get(1).is_some_and(|n| n.is(src, Kind::Punct, ",")) {
        (Some(name(2)?), 3)
    } else {
        (None, 1)
    };
    if !t.get(at)?.is(src, Kind::Ident, "in") {
        return None;
    }
    Some((item, index, t.get(at + 1)?.start))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(src: &str) -> Vec<(Kind, &str)> {
        tokens(src).iter().map(|t| (t.kind, t.text(src))).collect()
    }

    fn roots(src: &str) -> Vec<String> {
        chains(src).into_iter().map(|(p, _)| p.join(".")).collect()
    }

    #[test]
    fn derived_values() {
        let src = "let n = 0\nconst double = $derived(n * 2)\nlet ready = $derived(\n  guess.length === 5\n)\nx.$derived(1)";
        let out = derived(src).unwrap();
        assert_eq!(
            out,
            "let n = 0\nlet double = __wisp_d(() => (n * 2), (__v) => double = __v)\n\
             let ready = __wisp_d(() => (\n  guess.length === 5\n), (__v) => ready = __v)\nx.$derived(1)"
        );
        assert_eq!(out.lines().count(), src.lines().count());
        for bad in [
            "f($derived(1))",
            "let x = $derived",
            "let x = $derived()",
            "{ let y = $derived(1) }",
        ] {
            let (off, msg) = derived(bad).unwrap_err();
            assert!(
                bad[off..].starts_with("$derived") && msg.contains("$derived"),
                "{bad}: {msg}"
            );
        }
    }

    #[test]
    fn regex_or_division() {
        assert_eq!(
            kinds("a / b / c")
                .iter()
                .filter(|(k, _)| *k == Kind::Regex)
                .count(),
            0
        );
        assert_eq!(
            kinds("x = /a\\/b[/]c/g.test(y)")[2],
            (Kind::Regex, "/a\\/b[/]c/g")
        );
        assert_eq!(kinds("f(x) / 2")[4], (Kind::Punct, "/"));
        assert_eq!(kinds("return /re/")[1], (Kind::Regex, "/re/"));
        assert_eq!(kinds("a.return / 2")[3], (Kind::Punct, "/"));
        assert_eq!(
            kinds("if (/^[a-z]$/.test(k)) {}")[2],
            (Kind::Regex, "/^[a-z]$/")
        );
        assert_eq!(roots("n = a / b; m = /c/ / d"), ["n", "a", "b", "m", "d"]);
    }

    #[test]
    fn template_literals_nest() {
        let src = "`a ${b + `c ${d.e} f`} g ${ {h: i}.h }` + j";
        assert_eq!(roots(src), ["b", "d.e", "i", "j"]);
        let t = tokens(src);
        assert_eq!(t.last().unwrap().depth, 0);
        assert_eq!(kinds("`x`")[0], (Kind::Template, "`x`"));
        // A `}` of an object inside `${…}` is not the substitution's end.
        assert_eq!(roots("`${ {a: 1}[k] }${z}`"), ["k", "z"]);
    }

    #[test]
    fn comments_and_strings_hide_their_contents() {
        assert_eq!(
            roots("a // b's\n/* c \"d */ e 'f' \"g\" h"),
            ["a", "e", "h"]
        );
        assert_eq!(roots("x = 'it\\'s' + y"), ["x", "y"]);
    }

    #[test]
    fn chains_stop_at_calls_and_indexes() {
        assert_eq!(roots("key.mark.class()"), ["key.mark"]);
        assert_eq!(roots("data.items[0].name"), ["data.items"]);
        assert_eq!(roots("data.user?.name"), ["data.user"]);
        assert_eq!(roots("press(key.letter)"), ["press", "key.letter"]);
        assert_eq!(roots("[...rest, this.x, #p in o]"), ["rest", "o"]);
    }

    #[test]
    fn names_bound_inside_are_not_read() {
        assert_eq!(
            roots("function f({ data }) { return data.a } data.b"),
            ["data.b"]
        );
        assert_eq!(
            roots("xs.map((data, i) => data.x + i).concat(data.y)"),
            ["xs", "data.y"]
        );
        assert_eq!(roots("xs.map(data => data.x, data.z)"), ["xs", "data.z"]);
        assert_eq!(roots("f(({ a: data = d }) => { data.q })"), ["f", "d"]);
        assert_eq!(roots("try {} catch ({ data }) { data.x }"), [] as [&str; 0]);
        assert_eq!(
            roots("{ let data = 1, [e] = g; data.x + e } data.y"),
            ["g", "data.y"]
        );
        assert_eq!(roots("({ go(data) { return data } }).go(data)"), ["data"]);
        assert_eq!(roots("for (const data of list) f(data)"), ["list", "f"]);
        assert_eq!(roots("if (data.ok) { data.x }"), ["data.ok", "data.x"]);
        assert_eq!(
            roots("() => { function data() {} data() }; data.w"),
            ["data.w"]
        );
    }

    #[test]
    fn initializers() {
        let src = "let a = 1, b = [1,\n 2]\nconst c = data.x\nfoo()\nlet d";
        assert_eq!(initializer(src, "a"), Some("1"));
        assert_eq!(initializer(src, "c"), Some("data.x"));
        assert_eq!(initializer(src, "d"), None);
        assert_eq!(initializer(src, "b"), None); // not first in its statement
        assert_eq!(initializer("let s = 'x';", "s"), Some("'x'"));
    }

    #[test]
    fn keys_are_not_variables() {
        assert_eq!(
            roots("({ a: b, c, d: e ? f : g })"),
            ["b", "c", "e", "f", "g"]
        );
        assert_eq!(roots("x ? y : z"), ["x", "y", "z"]);
        assert_eq!(roots("switch (v) { case w: break }"), ["v", "w"]);
        assert_eq!(roots("{ label: 1 }"), [] as [&str; 0]);
    }

    #[test]
    fn top_level_declarations() {
        let src = "let a = 1, b = f(x, y)\nconst c = { d: 1 }\nfunction e() { let inner }\nclass F {}\nasync function* g() {}\nvar h\nfoo, bar\nif (x) { let no }\nconst k = function named() {}\nconst { l, m: n = o, ...p } = q, [r] = s";
        let names: Vec<String> = declarations(src).into_iter().map(|(n, _)| n).collect();
        assert_eq!(
            names,
            ["a", "b", "c", "e", "F", "g", "h", "k", "l", "n", "p", "r"]
        );
    }

    #[test]
    fn imports_are_found() {
        let src = "import a from 'a';\nimport { b,\n  c } from \"bc\"\nimport 'side'\nimport j from './j.json' with { type: 'json' };\nconst m = import('lazy')\nimport.meta.url\nfoo()";
        let spans: Vec<&str> = imports(src).iter().map(|&(s, e)| &src[s..e]).collect();
        assert_eq!(
            spans,
            [
                "import a from 'a';",
                "import { b,\n  c } from \"bc\"",
                "import 'side'",
                "import j from './j.json' with { type: 'json' };"
            ]
        );
        let blanked = blank(src, &imports(src));
        assert_eq!(blanked.lines().count(), src.lines().count());
        assert!(blanked.starts_with("                  \n") && blanked.contains("const m"));
    }

    #[test]
    fn handler_shapes() {
        assert!(
            is_path("toggle")
                && is_path("menu.close")
                && !is_path("a()")
                && !is_path("a.b.")
                && !is_path("this.x")
        );
        assert!(
            is_statements("a(); b()")
                && is_statements("if (x) y()")
                && !is_statements("count++")
                && !is_statements("f(() => { a; b })")
        );
        assert!(
            ends_in_line_comment("a // b")
                && !ends_in_line_comment("a // b\n + c")
                && !ends_in_line_comment("'//'")
        );
    }

    #[test]
    fn each_syntax() {
        assert_eq!(each("item in data.list"), Some(("item".into(), None, 8)));
        assert_eq!(each("x, i in xs"), Some(("x".into(), Some("i".into()), 8)));
        assert_eq!(each("x of xs"), None);
        assert_eq!(each("x in"), None);
        assert_eq!(each("if in xs"), None);
    }
}
