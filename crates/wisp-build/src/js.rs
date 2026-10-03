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

/// A browser global a page's code may mean even when its Rust has a name
/// like it (`let location = …`): that one is read as `data.location`.
pub fn is_global(word: &str) -> bool {
    matches!(
        word,
        "window"
            | "document"
            | "console"
            | "location"
            | "history"
            | "navigator"
            | "event"
            | "fetch"
            | "alert"
            | "confirm"
            | "prompt"
            | "performance"
            | "globalThis"
    )
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
    (end + 1).min(t.len())
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
            "catch" if is(k + 1, "(") => function(k + 1, &mut scopes),
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

/// `src` with the module name of each import (`import … from '…'`,
/// `export … from '…'`, `import('…')`) replaced by what `f` makes of it
/// (`None`: left as written). The new name is written as a JSON string.
pub fn specifiers(
    src: &str,
    mut f: impl FnMut(&str) -> Result<Option<String>, String>,
) -> Result<String, String> {
    let t = tokens(src);
    let mut out = String::with_capacity(src.len());
    let mut at = 0;
    for k in 0..t.len() {
        if t[k].kind != Kind::String {
            continue;
        }
        let prev = |n: usize| k.checked_sub(n).map(|j| t[j].text(src));
        if !(matches!(prev(1), Some("from" | "import"))
            || (prev(1) == Some("(") && prev(2) == Some("import")))
        {
            continue;
        }
        let raw = t[k].text(src);
        if raw.len() < 2 {
            continue;
        }
        let Some(url) = f(&raw[1..raw.len() - 1])? else {
            continue;
        };
        out.push_str(&src[at..t[k].start]);
        out.push_str(&crate::json_str(&url));
        at = t[k].end;
    }
    out.push_str(&src[at..]);
    Ok(out)
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

/// The runes a script may use, each where it goes.
const RUNES: [&str; 7] = [
    "$state",
    "$derived",
    "$effect",
    "$props",
    "$bindable",
    "$inspect",
    "$host",
];

/// What the runtime makes of a script's names, for `rewrite`.
#[derive(Debug, Default, Clone)]
pub struct Reactive {
    /// Signals, read and written as `name.v`: `let`s, `$state`s, `$derived`
    /// values, and the server values or props.
    pub state: Vec<String>,
    /// The `$derived` ones, which nothing may assign to.
    pub derived: Vec<String>,
    /// Names whose `$name` reads the store's value: `$cart` is `cart.value`.
    pub stores: Vec<String>,
}

/// A component's `let { a, b = 1, c: d, e = $bindable(), ...rest } =
/// $props()`: its span in the script, its props and the name of the rest.
#[derive(Debug, PartialEq)]
pub struct PropsRune {
    pub span: (usize, usize),
    pub props: Vec<RuneProp>,
    pub rest: Option<String>,
}

/// One prop of `$props()`: its name, the variable it is read as (`c: d`
/// reads prop `c` as `d`), its default (JavaScript) and whether a parent
/// may `bind:` it.
#[derive(Debug, PartialEq)]
pub struct RuneProp {
    pub name: String,
    pub local: String,
    pub default: Option<String>,
    pub bindable: bool,
}

/// The rune called by the whole of `t[s..e]`, as `$name` or `$name.member`,
/// with the tokens of its parentheses: `$state(0)` is `("$state", s + 1, e - 1)`.
fn rune_call<'a>(src: &'a str, t: &[Token], s: usize, e: usize) -> Option<(&'a str, usize, usize)> {
    let first = t.get(s)?;
    if first.kind != Kind::Ident || first.member || !first.text(src).starts_with('$') {
        return None;
    }
    let dotted = t.get(s + 1)?.is(src, Kind::Punct, ".") && t.get(s + 2)?.kind == Kind::Ident;
    let open = if dotted { s + 3 } else { s + 1 };
    if !t.get(open)?.is(src, Kind::Punct, "(") || close(t, open) + 1 != e {
        return None;
    }
    let end = if dotted { t[s + 2].end } else { first.end };
    Some((&src[first.start..end], open, e - 1))
}

/// Where the expression starting at token `s` ends, at the top level: at a
/// `,` or `;`, or at a line that starts another statement.
fn expr_end(src: &str, t: &[Token], s: usize) -> usize {
    (s + 1..t.len())
        .find(|&j| {
            let prev = t[j - 1];
            t[j].depth == 0
                && (matches!(t[j].text(src), "," | ";")
                    || (t[j].newline
                        && t[j].kind != Kind::Punct
                        && prev.depth == 0
                        && (prev.kind != Kind::Punct || matches!(prev.text(src), ")" | "]" | "}"))))
        })
        .unwrap_or(t.len())
}

/// An identifier written `{ name }` in an object or pattern, for `name: name`;
/// not in a block, such as a function's body.
fn shorthand(src: &str, t: &[Token], k: usize) -> bool {
    if !(t[k].inner == b'{'
        && k > 0
        && matches!(t[k - 1].text(src), "{" | ",")
        && t.get(k + 1)
            .is_some_and(|n| matches!(n.text(src), "," | "}" | "=")))
    {
        return false;
    }
    let open = (0..k).rev().find(|&j| t[j].depth < t[k].depth).unwrap_or(0);
    open.checked_sub(1).is_some_and(|p| {
        t[p].kind == Kind::String
            || match t[p].text(src) {
                ")" | ";" | "{" | "}" | "=>" | "else" | "do" | "try" | "finally" => false,
                // `case x: {` opens a block; `a ? b : {` and `k: {` an object.
                ":" => !(0..p)
                    .rev()
                    .take_while(|&j| t[j].depth >= t[p].depth)
                    .filter(|&j| t[j].depth == t[p].depth)
                    .map(|j| t[j].text(src))
                    .find(|w| matches!(*w, "case" | "default" | "?" | "{" | "," | ";" | "}"))
                    .is_some_and(|w| w == "case" || w == "default"),
                _ => true,
            }
    })
}

/// Whether the name at `k` is assigned to (`=`, `+=`, `++`, ...).
fn written(src: &str, t: &[Token], k: usize) -> bool {
    // `(x) = v` writes x too.
    let (mut a, mut b) = (k, k + 1);
    let text = |j: usize| t.get(j).map_or("", |n| n.text(src));
    while a > 0 && text(a - 1) == "(" && text(b) == ")" {
        (a, b) = (a - 1, b + 1);
    }
    let next = text(b);
    let prev = a.checked_sub(1).map_or("", text);
    matches!(prev, "++" | "--")
        || matches!(next, "++" | "--")
        || (next.ends_with('=')
            && !matches!(next, "==" | "===" | "!=" | "!==" | "<=" | ">=" | "=>"))
}

/// A string, number, boolean or null literal: a value nothing can change
/// inside.
fn is_primitive(src: &str, tok: Token) -> bool {
    match tok.kind {
        Kind::Number | Kind::String => true,
        Kind::Template => !tok.text(src).contains("${"),
        Kind::Ident => matches!(tok.text(src), "true" | "false" | "null"),
        _ => false,
    }
}

/// Whether the variable `name` of a script is assigned to, in `src` (its
/// tokens `t`, declared at `decl`) or in any of `more`, other than where
/// something inside declares a name of its own.
fn assigned(name: &str, decl: usize, src: &str, t: &[Token], more: &[&str]) -> bool {
    let hit = |s: &str, t: &[Token], skip: usize| {
        let own = bound(s, t);
        (0..t.len()).any(|k| {
            k != skip
                && t[k].kind == Kind::Ident
                && !t[k].member
                && !t[k].key
                && !own[k]
                && t[k].text(s) == name
                && written(s, t, k)
        })
    };
    hit(src, t, decl) || more.iter().any(|m| hit(m, &tokens(m), usize::MAX))
}

/// A component's `$props()`, if its script has one. It must be the
/// destructuring declaration of the props, at the top of the script.
pub fn props_rune(src: &str) -> Result<Option<PropsRune>, (usize, String)> {
    let t = tokens(src);
    let found: Vec<usize> = (0..t.len())
        .filter(|&k| !t[k].member && t[k].is(src, Kind::Ident, "$props"))
        .collect();
    let Some(&k) = found.first() else {
        return Ok(None);
    };
    let usage = "`$props()` names a component's props at the top of its script: `let { title, count = 0 } = $props()`";
    if let Some(&again) = found.get(1) {
        return Err((t[again].start, "a component has one `$props()`".into()));
    }
    let is = |j: usize, s: &str| {
        t.get(j)
            .is_some_and(|n| n.kind == Kind::Punct && n.text(src) == s)
    };
    let open = (k >= 3 && is(k - 1, "=") && is(k - 2, "}") && t[k - 2].depth == 0)
        .then(|| (0..k - 2).rev().find(|&j| t[j].depth == 0 && is(j, "{")))
        .flatten()
        .filter(|&o| o > 0 && matches!(t[o - 1].text(src), "let" | "const") && t[o - 1].depth == 0);
    let Some(open) = open.filter(|_| is(k + 1, "(") && is(k + 2, ")")) else {
        return Err((t[k].start, usage.into()));
    };
    let mut props: Vec<RuneProp> = Vec::new();
    let mut rest = None;
    let name_at = |j: usize| {
        t.get(j)
            .filter(|n| {
                n.kind == Kind::Ident
                    && !is_reserved(n.text(src))
                    && !n.text(src).starts_with(['$', '#'])
            })
            .map(|n| n.text(src).to_string())
    };
    let mut a = open + 1;
    while a < k - 2 {
        let b = (a..k - 2)
            .find(|&j| t[j].depth == 1 && is(j, ","))
            .unwrap_or(k - 2);
        if rest.is_some() {
            return Err((t[a].start, "`...rest` comes last in `$props()`".into()));
        }
        if is(a, "...") {
            match name_at(a + 1).filter(|_| b == a + 2) {
                Some(r) => rest = Some(r),
                None => return Err((t[a].start, "`...` takes a name: `...rest`".into())),
            }
            a = b + 1;
            continue;
        }
        // A reserved word is a prop's name only when read as another:
        // `class: cls`.
        let name = match name_at(a) {
            Some(n) => n,
            None if t[a].kind == Kind::Ident && is(a + 1, ":") => t[a].text(src).to_string(),
            None => return Err((t[a].start, usage.into())),
        };
        // `name: local` reads the prop as another variable.
        let (local, v) = if is(a + 1, ":") {
            match name_at(a + 2) {
                Some(l) => (l, a + 3),
                None => {
                    return Err((
                        t[a].start,
                        format!("`{name}:` takes the name to read it as: `{name}: other`"),
                    ));
                }
            }
        } else {
            (name.clone(), a + 1)
        };
        let (default, bindable) = if b == v {
            (None, false)
        } else if is(v, "=") && v + 1 < b {
            match rune_call(src, &t, v + 1, b) {
                Some(("$bindable", o, c)) => (
                    (c > o + 1).then(|| src[t[o + 1].start..t[c - 1].end].to_string()),
                    true,
                ),
                _ => (Some(src[t[v + 1].start..t[b - 1].end].to_string()), false),
            }
        } else {
            return Err((t[a].start, usage.into()));
        };
        if props.iter().any(|p| p.name == name) {
            return Err((t[a].start, format!("`{name}` is in `$props()` twice")));
        }
        props.push(RuneProp {
            name,
            local,
            default,
            bindable,
        });
        a = b + 1;
    }
    let end = if is(k + 3, ";") {
        t[k + 3].end
    } else {
        t[k + 2].end
    };
    Ok(Some(PropsRune {
        span: (t[open - 1].start, end),
        props,
        rest,
    }))
}

/// What a top-level declaration first sets a name to, as the first paint
/// sees it: `$state(x)` is `x`; another rune is nothing it can know.
pub fn plain_init(init: &str) -> Option<&str> {
    let t = tokens(init);
    match rune_call(init, &t, 0, t.len()) {
        Some(("$state" | "$state.raw", o, c)) if c > o + 1 => {
            Some(&init[t[o + 1].start..t[c - 1].end])
        }
        Some(_) => None,
        None if t.first().is_some_and(|f| RUNES.contains(&f.text(init))) => None,
        None => Some(init),
    }
}

/// The local names a script's `import` statements (as byte ranges) bind.
pub fn import_names(src: &str, spans: &[(usize, usize)]) -> Vec<String> {
    let mut out = Vec::new();
    for &(a, b) in spans {
        let s = &src[a..b];
        let t = tokens(s);
        for k in 0..t.len() {
            let w = t[k].text(s);
            if t[k].kind == Kind::Ident
                && !t[k].key
                && !matches!(w, "import" | "from" | "as" | "with" | "assert")
                && !t.get(k + 1).is_some_and(|n| n.text(s) == "as")
            {
                out.push(w.to_string());
            }
        }
    }
    out
}

/// Replaces byte ranges of `src`. An insertion is an empty range, and goes
/// before a replacement that starts where it is.
fn apply(src: &str, mut edits: Vec<(usize, usize, String)>) -> String {
    edits.sort_by_key(|e| (e.0, e.1));
    let mut out = String::with_capacity(src.len() + edits.len() * 8);
    let mut at = 0;
    for (s, e, text) in edits {
        // Code the scan misread can give overlapping edits: the first wins.
        if s < at {
            continue;
        }
        out.push_str(&src[at..s]);
        out.push_str(&text);
        at = e;
    }
    out.push_str(&src[at..]);
    out
}

/// The edits that make `t`'s free names read the runtime's signals and
/// stores: `count` is `count.v` (`{ count }` is `{ count: count.v }`), and
/// `$cart` is `cart.value`. Tokens in `skip` are left.
fn refs(
    src: &str,
    t: &[Token],
    r: &Reactive,
    skip: &[usize],
    edits: &mut Vec<(usize, usize, String)>,
) -> Result<(), (usize, String)> {
    let own = bound(src, t);
    for k in 0..t.len() {
        let tok = t[k];
        if tok.kind != Kind::Ident || tok.member || tok.key || own[k] || skip.contains(&k) {
            continue;
        }
        let word = tok.text(src);
        let (name, value) = match word.strip_prefix('$') {
            Some(base)
                if r.stores.iter().any(|s| s == base) && !r.state.iter().any(|s| s == word) =>
            {
                (base, ".value")
            }
            _ => (word, ""),
        };
        let signal = r.state.iter().any(|s| s == name);
        if !signal && value.is_empty() {
            continue;
        }
        if r.derived.iter().any(|d| d == name) && written(src, t, k) {
            return Err((
                tok.start,
                format!(
                    "`{name}` is `$derived`: it follows its expression, so change what that reads instead"
                ),
            ));
        }
        let read = format!("{name}{}{value}", if signal { ".v" } else { "" });
        let text = if shorthand(src, t, k) && value.is_empty() {
            format!("{word}: {read}")
        } else {
            read
        };
        edits.push((tok.start, tok.end, text));
    }
    Ok(())
}

/// A script as live.js runs it, and what `rewrite` needs for the file's
/// directives. `params` are the server values or props it gets, as signals.
///
/// - A top-level `let` (or `var`) is state: `let n = 0` is `let n =
///   __wisp_s(0)`, and every read and write of `n` is `n.v`. Objects and
///   arrays in it are deep: a change inside them is a change.
/// - `$state(x)` is the same, spelled out, and works with `const` too;
///   `$state.raw(x)` is not deep. `$state.snapshot(x)` is a plain copy.
/// - `$derived(expr)` and `$derived.by(fn)`: a value worked out from others.
/// - `$effect(fn)` and `$effect.pre(fn)`: code that runs again when what
///   it read changes. `$inspect(a, b)` logs them as they change, in
///   development; in release it is gone.
/// - `$name`, for a store `name`, is `name.value`.
///
/// Line breaks stay where they were. A rune somewhere else is an error,
/// with its offset.
pub fn script(
    src: &str,
    params: &[String],
    release: bool,
    extra: &[&str],
) -> Result<(String, Reactive), (usize, String)> {
    let t = tokens(src);
    let mut edits: Vec<(usize, usize, String)> = Vec::new();
    let mut skip: Vec<usize> = Vec::new(); // declared names, and runes handled
    let mut r = Reactive {
        state: params.to_vec(),
        ..Reactive::default()
    };
    let is = |j: usize, s: &str| {
        t.get(j)
            .is_some_and(|n| n.kind == Kind::Punct && n.text(src) == s)
    };
    let mut k = 0;
    while k < t.len() {
        let tok = t[k];
        let kw = tok.text(src);
        if tok.depth != 0
            || tok.member
            || tok.kind != Kind::Ident
            || !matches!(kw, "let" | "const" | "var")
        {
            k += 1;
            continue;
        }
        let is_let = kw != "const";
        let mut j = k + 1;
        loop {
            let mut names = Vec::new();
            let after = pattern(src, &t, j, &mut names);
            if names.is_empty() {
                break;
            }
            let simple = after == j + 1;
            let init = (is(after, "=") && after + 1 < t.len())
                .then(|| (after + 1, expr_end(src, &t, after + 1)));
            let end = init.map_or(after, |(_, e)| e);
            let rune = init.and_then(|(s, e)| rune_call(src, &t, s, e));
            let name = t[j].text(src);
            match rune {
                Some((
                    kind @ ("$derived" | "$derived.by" | "$state" | "$state.raw"),
                    open,
                    close,
                )) => {
                    let s = after + 1;
                    if !simple {
                        return Err((
                            t[s].start,
                            format!("`{kind}` sets one name: `let name = {kind}(…)`"),
                        ));
                    }
                    let derived = kind.starts_with("$derived");
                    if derived && close == open + 1 {
                        return Err((
                            t[s].start,
                            format!("`{kind}(…)` takes what the value is worked out from"),
                        ));
                    }
                    let (head, tail) = match kind {
                        "$derived" => ("__wisp_d(() => (", "))"),
                        "$derived.by" => ("__wisp_d(", ")"),
                        "$state" => ("__wisp_s(", ")"),
                        _ => ("__wisp_r(", ")"),
                    };
                    edits.push((t[s].start, t[open].end, head.into()));
                    edits.push((t[close].start, t[close].end, tail.into()));
                    skip.extend([j, s]);
                    r.state.push(name.into());
                    if derived {
                        r.derived.push(name.into());
                    }
                }
                _ if !is_let => {}
                // Set once to a string, number, boolean or null and never
                // again (here or in `extra`): a constant, not a signal.
                None if simple
                    && init.is_some_and(|(s, e)| e == s + 1 && is_primitive(src, t[s]))
                    && !assigned(name, j, src, &t, extra) =>
                {
                    skip.push(j);
                }
                _ if simple => {
                    skip.push(j);
                    r.state.push(name.into());
                    match init {
                        Some((s, e)) => {
                            edits.push((t[s].start, t[s].start, "__wisp_s(".into()));
                            edits.push((t[e - 1].end, t[e - 1].end, ")".into()));
                        }
                        None => edits.push((t[j].end, t[j].end, " = __wisp_s()".into())),
                    }
                }
                _ => {
                    // `let { a, b: c } = x` binds hidden names, then the state.
                    let mut tail = String::new();
                    for &n in &names {
                        let name = t[n].text(src);
                        let text = if shorthand(src, &t, n) {
                            format!("{name}: __wisp_{name}")
                        } else {
                            format!("__wisp_{name}")
                        };
                        edits.push((t[n].start, t[n].end, text));
                        tail.push_str(&format!(", {name} = __wisp_s(__wisp_{name})"));
                        skip.push(n);
                        r.state.push(name.into());
                    }
                    let at = t[end - 1].end;
                    edits.push((at, at, tail));
                }
            }
            if !(is(end, ",") && t[end].depth == 0) {
                j = end;
                break;
            }
            j = end + 1;
        }
        k = j.max(k + 1);
    }

    // A class's `count = $state(0)` field is a private signal behind a
    // getter and setter (a `$derived` one, a getter), so its instances are
    // state too.
    for k in 0..t.len() {
        if t[k].member || !t[k].is(src, Kind::Ident, "class") {
            continue;
        }
        let Some(open) = (k + 1..t.len()).find(|&j| t[j].depth == t[k].depth && is(j, "{")) else {
            continue;
        };
        let body = close(&t, open);
        let d = t[open].depth + 1;
        for j in open + 1..body {
            let f = t[j];
            let starts = f.depth == d
                && f.kind == Kind::Ident
                && !f.text(src).starts_with('#')
                && (j == open + 1 || f.newline || is(j - 1, ";"))
                && is(j + 1, "=");
            let p = if is(j + 3, "(") { j + 3 } else { j + 5 };
            if !starts || !is(p, "(") {
                continue;
            }
            let e = close(&t, p) + 1;
            if e > t.len() {
                continue;
            }
            let Some((kind, o, c)) = rune_call(src, &t, j + 2, e) else {
                continue;
            };
            let (head, tail, set) = match kind {
                "$state" => ("__wisp_s(", ")", true),
                "$state.raw" => ("__wisp_r(", ")", true),
                "$derived" if c > o + 1 => ("__wisp_d(() => (", "))", false),
                "$derived.by" if c > o + 1 => ("__wisp_d(", ")", false),
                _ => continue,
            };
            let name = f.text(src);
            let mut access = format!("; get {name}() {{ return this.#w_{name}.v }}");
            if set {
                access.push_str(&format!(" set {name}(v) {{ this.#w_{name}.v = v }}"));
            }
            edits.push((f.start, f.end, format!("#w_{name}")));
            edits.push((t[j + 2].start, t[o].end, head.into()));
            edits.push((t[c].start, t[c].end, format!("{tail}{access}")));
            skip.extend([j, j + 2]);
        }
    }

    // The other runes, where they may be.
    let mut k = 0;
    while k < t.len() {
        let tok = t[k];
        let word = tok.text(src);
        if tok.kind != Kind::Ident || tok.member || skip.contains(&k) || !RUNES.contains(&word) {
            k += 1;
            continue;
        }
        let member = (is(k + 1, ".") && t.get(k + 2).is_some_and(|n| n.kind == Kind::Ident))
            .then(|| t[k + 2].text(src));
        let callee_end = if member.is_some() { k + 3 } else { k + 1 };
        let called = is(callee_end, "(");
        let with = |name: &str| (tok.start, t[callee_end - 1].end, name.to_string());
        match (word, member) {
            ("$effect", None) if called => edits.push(with("__wisp_e")),
            ("$effect", Some("pre")) if called => edits.push(with("__wisp_ep")),
            ("$state", Some("snapshot")) if called => edits.push(with("__wisp_snap")),
            ("$inspect", None) if called => {
                let c = close(&t, callee_end);
                if c >= t.len() {
                    return Err((tok.start, "`$inspect(…)` is not closed".into()));
                }
                if is(c + 1, ".") {
                    return Err((
                        tok.start,
                        "`$inspect(…)` logs what it is given; `.with` is not supported".into(),
                    ));
                }
                if release {
                    // Gone, but its line breaks stay.
                    let lines = src[tok.start..t[c].end].matches('\n').count();
                    edits.push((tok.start, t[c].end, format!("void 0{}", "\n".repeat(lines))));
                    skip.extend(k..=c);
                    k = c + 1;
                    continue;
                }
                edits.push((
                    tok.start,
                    t[callee_end].end,
                    "__wisp_e(() => console.log(...[".into(),
                ));
                edits.push((t[c].start, t[c].end, "].map(__wisp_snap)))".into()));
            }
            ("$state", _) => {
                return Err((tok.start, "`$state(…)` declares state at the top of the script: `let name = $state(value)`".into()));
            }
            ("$derived", _) => {
                return Err((
                    tok.start,
                    "`$derived(…)` declares a value at the top of the script: `let name = $derived(expression)`".into(),
                ));
            }
            ("$props" | "$bindable", _) => {
                return Err((
                    tok.start,
                    "`$props()` names a component's props at the top of its script: `let { title, count = $bindable(0) } = $props()`".into(),
                ));
            }
            ("$effect", _) => {
                return Err((
                    tok.start,
                    "`$effect(fn)` and `$effect.pre(fn)` run fn again when what it reads changes"
                        .into(),
                ));
            }
            ("$inspect", _) => {
                return Err((
                    tok.start,
                    "`$inspect(a, b)` logs values as they change".into(),
                ));
            }
            _ => {
                return Err((
                    tok.start,
                    format!(
                        "`{word}` is not a rune in Wisp: there are $state, $derived, $effect, $props and $inspect"
                    ),
                ));
            }
        }
        k += 1;
    }

    let mut stores = import_names(src, &imports(src));
    stores.extend(declarations(src).into_iter().map(|(n, _)| n));
    stores.extend(params.iter().cloned());
    r.stores = stores;
    refs(src, &t, &r, &skip, &mut edits)?;
    Ok((apply(src, edits), r))
}

/// A directive's JavaScript as it runs: its script's names read as
/// `script` made them. A rune here is an error: runes go in the script.
pub fn rewrite(src: &str, r: &Reactive) -> Result<String, (usize, String)> {
    let t = tokens(src);
    if let Some(tok) = t
        .iter()
        .find(|n| n.kind == Kind::Ident && !n.member && RUNES.contains(&n.text(src)))
    {
        return Err((
            tok.start,
            format!("`{}` goes in the script, not in markup", tok.text(src)),
        ));
    }
    let mut edits = Vec::new();
    let skip = selects(src, &t, r, &mut edits);
    refs(src, &t, r, &skip, &mut edits)?;
    Ok(apply(src, edits))
}

/// The edits that make `x === e` (and `!==`), for a state variable `x`,
/// `__wisp_eq(x, e)`: what reads it then runs again only when the answer
/// may change, when `x` becomes `e` or stops being it (see live.js's `eq`).
/// So `class:on="selected === row.id"` redraws two rows of a list, not all.
/// Only where nothing before `x` binds tighter, and `e` ends on its line.
/// The tokens of the `x`s are returned.
fn selects(
    src: &str,
    t: &[Token],
    r: &Reactive,
    edits: &mut Vec<(usize, usize, String)>,
) -> Vec<usize> {
    let own = bound(src, t);
    let punct = |j: usize, set: &[&str]| t[j].kind == Kind::Punct && set.contains(&t[j].text(src));
    let mut out = Vec::new();
    for k in 0..t.len().saturating_sub(2) {
        let (tok, op) = (t[k], t[k + 1]);
        let name = tok.text(src);
        let open = k == 0
            || punct(
                k - 1,
                &[
                    "(", "[", "{", ",", ";", "?", ":", "&&", "||", "??", "=>", "=",
                ],
            )
            || t[k - 1].is(src, Kind::Ident, "return");
        if tok.kind != Kind::Ident
            || tok.member
            || tok.key
            || own[k]
            || !open
            || !punct(k + 1, &["===", "!=="])
            || !r.state.iter().any(|s| s == name)
            || r.derived.iter().any(|d| d == name)
        {
            continue;
        }
        let d = tok.depth;
        let loose = [
            "&&", "||", "??", "?", ":", ",", ";", "==", "!=", "===", "!==", "&", "|", "^", "=",
            "=>",
        ];
        let mut e = k + 2;
        while e < t.len()
            && t[e].depth >= d
            && !(t[e].depth == d && (punct(e, &loose) || t[e].newline))
        {
            e += 1;
        }
        // A new line goes on the expression, unless a statement starts there.
        let split = t.get(e).is_some_and(|n| {
            let word = n.kind == Kind::Ident && !matches!(n.text(src), "in" | "instanceof");
            n.depth == d && n.newline && !word && !punct(e, &loose)
        });
        if e == k + 2 || split {
            continue;
        }
        let not = if op.text(src) == "!==" { "!" } else { "" };
        edits.push((tok.start, op.end, format!("{not}__wisp_eq({name},")));
        edits.push((t[e - 1].end, t[e - 1].end, ")".into()));
        out.push(k);
    }
    out
}

/// `src` without its comments and the whitespace JavaScript does not need,
/// for what release builds serve. A line break stays only where taking it
/// out could join two statements (after a value, before one), so no
/// semicolon the source leaves out is lost.
pub fn minify(src: &str) -> String {
    let mut out = String::with_capacity(src.len() / 2);
    let mut prev: Option<Token> = None;
    for tok in tokens(src) {
        let text = tok.text(src);
        if let Some(p) = prev {
            let pt = p.text(src);
            let ends = matches!(
                p.kind,
                Kind::Ident | Kind::Number | Kind::String | Kind::Regex
            ) || (p.kind == Kind::Template && pt.ends_with('`'))
                || matches!(pt, ")" | "]" | "}" | "++" | "--");
            let starts = matches!(
                tok.kind,
                Kind::Ident | Kind::Number | Kind::String | Kind::Regex
            ) || (tok.kind == Kind::Template && text.starts_with('`'))
                || matches!(text, "++" | "--" | "!" | "~");
            let (l, f) = (pt.as_bytes()[pt.len() - 1], text.as_bytes()[0]);
            if tok.newline && ends && starts {
                out.push('\n');
            } else if (ident_byte(l) && (ident_byte(f) || f == b'#'))
                || (p.kind == Kind::Regex && ident_byte(f))
                // A number goes on through letters, digits, dots and an
                // exponent's sign.
                || (p.kind == Kind::Number
                    && (ident_byte(f) || f == b'.' || (matches!(l, b'e' | b'E') && matches!(f, b'+' | b'-'))))
                || (l == b'.' && f.is_ascii_digit())
                // Two marks that would read as one: `* *`, `+ +`, `/ /re/`.
                || (!ident_byte(l)
                    && !ident_byte(f)
                    && ([l, f] == *b"/*"
                        || [l, f] == *b"//"
                        || PUNCTS.iter().any(|p| p.as_bytes().windows(2).any(|w| w == [l, f]))))
            {
                out.push(' ');
            }
        }
        out.push_str(text);
        prev = Some(tok);
    }
    out
}

/// The runtime's own files as release builds serve them: without their
/// dev-only lines, `mangle`d, then `minify`d.
pub fn runtime(src: &str) -> String {
    minify(&mangle(&strip_dev(src)))
}

/// `src` without the lines from each `// dev{` line to the next `// }dev`
/// line, both included: what the runtime keeps for the devtools, which
/// debug builds serve as written and release builds never have.
pub fn strip_dev(src: &str) -> String {
    let mut out = String::with_capacity(src.len());
    let mut dev = false;
    for line in src.split_inclusive('\n') {
        match line.trim() {
            "// dev{" => dev = true,
            "// }dev" => dev = false,
            _ if !dev => out.push_str(line),
            _ => {}
        }
    }
    out
}

/// `src` with the names it binds shortened, for the runtime's own files:
/// each one not exported and never read unbound (as a global) becomes the
/// shortest name free, the most used first. A name is renamed alike
/// wherever it is bound, so every scope still sees what it saw. Properties,
/// object keys and method names stay; `{ a }` becomes `{ a: x }`, and
/// `const` is `let`. The code keeps to what this reads: no `eval`, `with`,
/// labels or class fields.
/// With `export { … }` or `export default`, `src` is left as it is.
pub fn mangle(src: &str) -> String {
    let t = tokens(src);
    let own = bound(src, &t);
    let top: Vec<String> = declarations(src).into_iter().map(|(n, _)| n).collect();
    let is = |j: usize, s: &str| {
        t.get(j)
            .is_some_and(|n| n.kind == Kind::Punct && n.text(src) == s)
    };
    let word = |j: usize| {
        t.get(j)
            .filter(|n| n.kind == Kind::Ident)
            .map(|n| n.text(src))
    };
    // What the module exports keeps its name.
    let mut kept: Vec<&str> = Vec::new();
    for k in (0..t.len()).filter(|&k| t[k].depth == 0 && word(k) == Some("export")) {
        match word(k + 1) {
            Some("const" | "let" | "var") => {
                let mut names = Vec::new();
                let mut j = pattern(src, &t, k + 2, &mut names);
                // More declarators after top-level commas.
                while j < t.len() && !is(j, ";") && !(t[j].newline && t[j].depth == 0) {
                    if t[j].depth == 0 && is(j, ",") {
                        j = pattern(src, &t, j + 1, &mut names);
                    } else {
                        j += 1;
                    }
                }
                kept.extend(names.iter().map(|&n| t[n].text(src)));
            }
            Some("function" | "class") => kept.extend(word(k + 2)),
            Some("async") => kept.extend(word(k + 3)),
            _ => return src.to_string(),
        }
    }
    // Each name's tokens, and whether each of them is bound where it is.
    let mut uses: std::collections::BTreeMap<&str, (Vec<usize>, bool)> = Default::default();
    for (j, n) in t.iter().enumerate() {
        let w = n.text(src);
        if n.kind != Kind::Ident || n.member || n.key || is_reserved(w) {
            continue;
        }
        // `get x()`, `static`, `async`: words; `name(…) {`: a method's name.
        let modifier = matches!(w, "get" | "set" | "static" | "async")
            && (word(j + 1).is_some() || is(j + 1, "[") || is(j + 1, "#"));
        let named = j.checked_sub(1).is_some_and(|p| {
            word(p) == Some("function") || (is(p, "*") && p > 0 && word(p - 1) == Some("function"))
        });
        let method = !named && is(j + 1, "(") && {
            let c = close(&t, j + 1);
            is(c + 1, "{") && !t[c + 1].newline
        };
        if modifier || method {
            continue;
        }
        let u = uses.entry(w).or_insert((Vec::new(), true));
        u.0.push(j);
        u.1 &= own[j] || top.iter().any(|x| x == w);
    }
    let taken = |w: &str| {
        is_reserved(w)
            || matches!(
                w,
                "with"
                    | "yield"
                    | "of"
                    | "as"
                    | "let"
                    | "get"
                    | "set"
                    | "static"
                    | "async"
                    | "from"
                    | "arguments"
                    | "eval"
                    | "undefined"
                    | "NaN"
                    | "Infinity"
            )
    };
    let mut names: Vec<(&str, &Vec<usize>)> = uses
        .iter()
        .filter(|(w, (_, ok))| *ok && !kept.contains(w) && !taken(w))
        .map(|(w, (at, _))| (*w, at))
        .collect();
    names.sort_by(|a, b| b.1.len().cmp(&a.1.len()).then(a.0.cmp(b.0)));
    // New names must not be a name that stays: a global, an export.
    let stays = |w: &str| taken(w) || uses.get(w).is_some_and(|_| !names.iter().any(|n| n.0 == w));
    const FIRST: &[u8] = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ_$";
    const NEXT: &[u8] = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ_$0123456789";
    let mut i = 0;
    let mut edits = Vec::new();
    for (w, at) in &names {
        let short = loop {
            let mut s = String::from(FIRST[i % FIRST.len()] as char);
            let mut n = i / FIRST.len();
            while n > 0 {
                n -= 1;
                s.push(NEXT[n % NEXT.len()] as char);
                n /= NEXT.len();
            }
            i += 1;
            if !stays(&s) {
                break s;
            }
        };
        for &j in at.iter() {
            let text = if shorthand(src, &t, j) {
                format!("{w}: {short}")
            } else {
                short.clone()
            };
            edits.push((t[j].start, t[j].end, text));
        }
    }
    // `const` does what `let` does here, in fewer bytes.
    for n in &t {
        if n.kind == Kind::Ident && !n.member && !n.key && n.text(src) == "const" {
            edits.push((n.start, n.end, "let".into()));
        }
    }
    apply(src, edits)
}

/// `src` with each `env.PUBLIC_NAME`, where `env` is no name of its own,
/// as the JavaScript string of `value(name)`: browser code's environment,
/// filled in when the app is built. Any other `env` is an error at its
/// offset: a name without `PUBLIC_` (so a secret cannot reach the
/// browser), one that is not set, or `env` read whole.
pub fn public_env(
    src: &str,
    value: &dyn Fn(&str) -> Option<String>,
) -> Result<String, (usize, String)> {
    if !src.contains("env") {
        return Ok(src.to_string());
    }
    let t = tokens(src);
    let mut top: Vec<String> = declarations(src).into_iter().map(|(n, _)| n).collect();
    top.extend(import_names(src, &imports(src)));
    if top.iter().any(|n| n == "env") {
        return Ok(src.to_string());
    }
    let own = bound(src, &t);
    let mut edits = Vec::new();
    for (k, tok) in t.iter().enumerate() {
        if tok.kind != Kind::Ident || tok.member || tok.key || own[k] || tok.text(src) != "env" {
            continue;
        }
        let name = t
            .get(k + 2)
            .filter(|n| n.kind == Kind::Ident && t[k + 1].is(src, Kind::Punct, "."));
        let Some(name) = name else {
            return Err((
                tok.start,
                "`env` is filled in when the app is built, a name at a time: `env.PUBLIC_API_URL`"
                    .into(),
            ));
        };
        let n = name.text(src);
        if !n.starts_with("PUBLIC_") {
            return Err((
                tok.start,
                format!(
                    "`env.{n}` is not sent to the browser: only `PUBLIC_` variables are, so that a secret cannot leak. \
                     Name it `PUBLIC_{n}` if it is public, or read it on the server with `wisp::env(\"{n}\")`"
                ),
            ));
        }
        let Some(v) = value(n) else {
            return Err((
                tok.start,
                format!(
                    "`env.{n}` is not set: set {n} in the environment or in .env (`{n}=…`; empty is allowed)"
                ),
            ));
        };
        edits.push((tok.start, name.end, crate::json_str(&v)));
    }
    Ok(apply(src, edits))
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

// ---- TypeScript ---------------------------------------------------------

const ENUM: &str = "`enum` is not erasable TypeScript (it makes code): use an object, \
    `const Color = { Red: 'red', Blue: 'blue' } as const`, and for its type \
    `type Color = (typeof Color)[keyof typeof Color]`";
const NAMESPACE: &str = "a `namespace` with values is not erasable TypeScript (it makes code): \
    use a module, such as src/lib/name.ts, or an object";
const PARAMETER_PROPERTY: &str = "a parameter property (`private x` in a constructor's parameters) \
    is not erasable TypeScript (it makes code): declare the field and set it, \
    `x: T; constructor(x: T) { this.x = x }`";

/// `src`, TypeScript, as JavaScript: its types gone the way Node's
/// `--experimental-strip-types` takes them out, each replaced by spaces, so
/// every line and column stays where it was (and a source map stays exact).
///
/// Gone: annotations, `interface`, `type`, `as` and `satisfies`, type
/// arguments and parameters, `!` after an operand, `declare`, `import type`
/// and `export type` (and `type` specifiers), `abstract`, access modifiers,
/// `implements`, optional `?`, overloads and index signatures. What
/// TypeScript would write code for (`enum`, a `namespace` with values,
/// parameter properties, `import x =`, `export =`) is an error, at its
/// offset, that says what to write instead.
pub fn strip_types(src: &str) -> Result<String, (usize, String)> {
    let t = split_closers(src, tokens(src));
    // Each token's bracket, and each closer's opener.
    let (mut open, mut mate) = (vec![usize::MAX; t.len()], vec![usize::MAX; t.len()]);
    let mut stack: Vec<usize> = Vec::new();
    for (k, tok) in t.iter().enumerate() {
        let text = tok.text(src);
        let piece = tok.kind == Kind::Template;
        if (tok.kind == Kind::Punct && matches!(text, "}" | ")" | "]"))
            || (piece && text.starts_with('}'))
        {
            mate[k] = stack.pop().unwrap_or(usize::MAX);
        }
        open[k] = stack.last().copied().unwrap_or(usize::MAX);
        if (tok.kind == Kind::Punct && matches!(text, "{" | "(" | "["))
            || (piece && text.ends_with("${"))
        {
            stack.push(k);
        }
    }
    let mut ts = Ts {
        src,
        gone: vec![false; t.len()],
        t,
        open,
        mate,
        classes: Vec::new(),
        cut: Vec::new(),
    };
    let mut k = 0;
    while k < ts.t.len() {
        k = ts.step(k)?;
    }
    let mut cut = ts.cut;
    cut.sort_unstable();
    let mut merged: Vec<(usize, usize)> = Vec::with_capacity(cut.len());
    for (a, b) in cut {
        match merged.last_mut() {
            Some(last) if a <= last.1 => last.1 = last.1.max(b),
            _ => merged.push((a, b)),
        }
    }
    Ok(blank(src, &merged))
}

/// `>>`, `>=` and the like as one token each of their characters, `>`
/// apart from `=`: a type's `<…>` may end inside one (`Map<K, Set<V>>`).
fn split_closers(src: &str, t: Vec<Token>) -> Vec<Token> {
    let mut out = Vec::with_capacity(t.len());
    for tok in t {
        let text = tok.text(src);
        if tok.kind != Kind::Punct || text.len() < 2 || !text.starts_with('>') {
            out.push(tok);
            continue;
        }
        let arrows = text.bytes().take_while(|&c| c == b'>').count();
        for i in 0..arrows {
            out.push(Token {
                start: tok.start + i,
                end: tok.start + i + 1,
                newline: tok.newline && i == 0,
                ..tok
            });
        }
        if arrows < text.len() {
            out.push(Token {
                start: tok.start + arrows,
                newline: false,
                ..tok
            });
        }
    }
    out
}

/// The stripper's state: the tokens, which of them are gone, and the byte
/// ranges to blank.
struct Ts<'a> {
    src: &'a str,
    t: Vec<Token>,
    gone: Vec<bool>,
    /// Per token: the bracket it is in, and a closer's opener (or `MAX`).
    open: Vec<usize>,
    mate: Vec<usize>,
    /// The `{` of each class body.
    classes: Vec<usize>,
    cut: Vec<(usize, usize)>,
}

type Step = Result<usize, (usize, String)>;

impl Ts<'_> {
    fn text(&self, j: usize) -> &str {
        self.t.get(j).map_or("", |n| n.text(self.src))
    }

    fn is(&self, j: usize, s: &str) -> bool {
        self.t
            .get(j)
            .is_some_and(|n| n.kind == Kind::Punct && n.text(self.src) == s)
    }

    /// An identifier `s`, not a property.
    fn word(&self, j: usize, s: &str) -> bool {
        self.t
            .get(j)
            .is_some_and(|n| n.kind == Kind::Ident && !n.member && n.text(self.src) == s)
    }

    fn ident(&self, j: usize) -> bool {
        self.t.get(j).is_some_and(|n| n.kind == Kind::Ident)
    }

    /// The index after the bracket at `j` and what it holds.
    fn after(&self, j: usize) -> usize {
        (close(&self.t, j) + 1).min(self.t.len())
    }

    /// Tokens `a..b` and the text they span, gone.
    fn erase(&mut self, a: usize, b: usize) {
        let b = b.min(self.t.len());
        if a < b {
            self.cut.push((self.t[a].start, self.t[b - 1].end));
            self.gone[a..b].fill(true);
        }
    }

    /// The last token before `k` that is not gone.
    fn prev(&self, k: usize) -> Option<usize> {
        (0..k).rev().find(|&j| !self.gone[j])
    }

    /// Whether token `p` can end an operand, so that a `<`, `!` or `as`
    /// after it is TypeScript's rather than an expression's start.
    fn ends_operand(&self, p: usize) -> bool {
        let tok = self.t[p];
        let text = tok.text(self.src);
        match tok.kind {
            Kind::Ident => {
                matches!(text, "this" | "super" | "true" | "false" | "null")
                    || !(is_reserved(text) || BEFORE_EXPRESSION.contains(&text))
            }
            Kind::Number | Kind::String | Kind::Regex => true,
            Kind::Template => text.ends_with('`'),
            Kind::Punct => match text {
                "]" | "}" => true,
                // Not the head of `if (…)`, `while (…)` and the like.
                ")" => {
                    let o = self.mate[p];
                    !(o > 0
                        && o != usize::MAX
                        && ["if", "while", "for", "with"]
                            .iter()
                            .any(|w| self.word(o - 1, w)))
                }
                _ => false,
            },
        }
    }

    fn operand(&self, k: usize) -> bool {
        self.prev(k).is_some_and(|p| self.ends_operand(p))
    }

    /// The index after the template literal starting at `j`.
    fn template_end(&self, j: usize) -> usize {
        if !self.text(j).ends_with("${") {
            return j + 1;
        }
        let d = self.t[j].depth;
        (j + 1..self.t.len())
            .find(|&i| {
                let n = self.t[i];
                let text = n.text(self.src);
                n.kind == Kind::Template
                    && n.depth == d
                    && text.starts_with('}')
                    && text.ends_with('`')
            })
            .map_or(self.t.len(), |i| i + 1)
    }

    /// The index after the `<…>` at `k`, if it closes before its bracket
    /// or statement does. `strict`: only if all inside can be types, so
    /// `a < b > c` stays two comparisons.
    fn angles(&self, k: usize, strict: bool) -> Option<usize> {
        let d = self.t[k].depth;
        let mut n = 0;
        for j in k..self.t.len() {
            let tok = self.t[j];
            let text = tok.text(self.src);
            if tok.depth < d
                || (tok.depth == d && text == ";")
                || (strict && tok.kind == Kind::Regex)
            {
                return None;
            }
            if tok.depth == d && text == "<" {
                n += 1;
            } else if tok.depth == d && text == ">" {
                n -= 1;
                if n == 0 {
                    return Some(j + 1);
                }
            } else if strict
                && tok.kind == Kind::Punct
                && !matches!(
                    text,
                    "." | ","
                        | "|"
                        | "&"
                        | "["
                        | "]"
                        | "{"
                        | "}"
                        | "("
                        | ")"
                        | "=>"
                        | "?"
                        | ":"
                        | ";"
                        | "="
                        | "..."
                        | "-"
                )
            {
                return None;
            }
        }
        None
    }

    /// The index after the type at `k`, or `k` without one.
    fn ty(&self, k: usize) -> usize {
        let mut j = k + usize::from(self.is(k, "|") || self.is(k, "&"));
        let mut end = k;
        loop {
            let e = self.ty_one(j);
            if e == j {
                return end;
            }
            end = e;
            // A conditional type: `T extends U ? X : Y`.
            if self.word(e, "extends") && !self.t[e].newline {
                let c = self.ty(e + 1);
                if self.is(c, "?") {
                    let a = self.ty(c + 1);
                    if self.is(a, ":") {
                        return self.ty(a + 1);
                    }
                }
                return end;
            }
            if !(self.is(e, "|") || self.is(e, "&")) {
                return end;
            }
            j = e + 1;
        }
    }

    /// The index after one type at `k`, without `|`, `&` or `extends`.
    fn ty_one(&self, k: usize) -> usize {
        let mut j = k;
        // Prefixes: `keyof T`, `typeof x`, `readonly T[]`, `infer U`,
        // `new () => T`, `asserts x is T`, `-1`.
        while let Some(n) = self.t.get(j) {
            let next = self.t.get(j + 1);
            let word = n.kind == Kind::Ident
                && matches!(
                    n.text(self.src),
                    "keyof"
                        | "typeof"
                        | "readonly"
                        | "unique"
                        | "infer"
                        | "asserts"
                        | "new"
                        | "abstract"
                )
                && next.is_some_and(|m| {
                    m.kind != Kind::Punct || matches!(m.text(self.src), "(" | "[" | "{" | "<")
                });
            let minus = self.is(j, "-") && next.is_some_and(|m| m.kind == Kind::Number);
            if !(word || minus) {
                break;
            }
            j += 1;
        }
        let Some(n) = self.t.get(j) else { return k };
        let mut function = false;
        let mut j = match n.kind {
            Kind::Ident => {
                let mut e = j + 1;
                while self.is(e, ".") && self.ident(e + 1) {
                    e += 2;
                }
                if self.word(j, "import") && self.is(e, "(") {
                    e = self.after(e);
                    while self.is(e, ".") && self.ident(e + 1) {
                        e += 2;
                    }
                }
                if self.is(e, "<") {
                    e = self.angles(e, false).unwrap_or(e);
                }
                // A predicate: `x is T`.
                if self.word(e, "is") && !self.t[e].newline {
                    e = self.ty(e + 1);
                }
                e
            }
            Kind::String | Kind::Number => j + 1,
            Kind::Template => self.template_end(j),
            Kind::Regex => return k,
            Kind::Punct => match n.text(self.src) {
                "{" | "[" => self.after(j),
                "(" => {
                    function = true;
                    self.after(j)
                }
                "<" => match self.angles(j, false) {
                    Some(a) if self.is(a, "(") => {
                        function = true;
                        self.after(a)
                    }
                    _ => return k,
                },
                _ => return k,
            },
        };
        if function && self.is(j, "=>") {
            return self.ty(j + 1);
        }
        while self.is(j, "[") && !self.t[j].newline {
            j = self.after(j);
        }
        j
    }

    /// The end of the declaration at `k` when it is all type (`type`,
    /// `interface`, `declare …`, a namespace of types), with its `;`.
    fn types_end(&self, k: usize) -> Result<Option<usize>, (usize, String)> {
        let tok = self.t[k];
        if tok.kind != Kind::Ident || tok.member || tok.key {
            return Ok(None);
        }
        let named = self.ident(k + 1) && !self.t[k + 1].newline;
        let e = match tok.text(self.src) {
            "type" if named && (self.is(k + 2, "=") || self.is(k + 2, "<")) => {
                let mut j = k + 2;
                if self.is(j, "<") {
                    j = self.angles(j, false).unwrap_or(j);
                }
                if !self.is(j, "=") {
                    return Ok(None);
                }
                self.ty(j + 1)
            }
            "interface"
                if named
                    && (self.is(k + 2, "{")
                        || self.is(k + 2, "<")
                        || self.word(k + 2, "extends")) =>
            {
                self.block_end(k)
            }
            "declare" => return Ok(self.declared(k)),
            "namespace" | "module" if named && (self.is(k + 2, "{") || self.is(k + 2, ".")) => {
                let b = self.block_end(k);
                let c = b.saturating_sub(1);
                let mut j = (k..c).find(|&j| self.is(j, "{")).map_or(c, |o| o + 1);
                // Each of its statements must be a type.
                while j < c {
                    j += usize::from(self.word(j, "export"));
                    if self.is(j, ";") {
                        j += 1;
                        continue;
                    }
                    match self.types_end(j)? {
                        Some(e) => j = e,
                        None => return Err((tok.start, NAMESPACE.into())),
                    }
                }
                return Ok(Some(b));
            }
            "enum" if named && self.is(k + 2, "{") => return Err((tok.start, ENUM.into())),
            _ => return Ok(None),
        };
        Ok(Some(e + usize::from(self.is(e, ";"))))
    }

    /// The index after the first `{…}` at `k`'s depth from `k` on, or a `;`
    /// that comes first.
    fn block_end(&self, k: usize) -> usize {
        let d = self.t[k].depth;
        match (k..self.t.len())
            .find(|&j| self.t[j].depth == d && (self.is(j, "{") || self.is(j, ";")))
        {
            Some(j) if self.is(j, "{") => self.after(j),
            Some(j) => j + 1,
            None => self.t.len(),
        }
    }

    /// The end of a `declare …` at `k`, or `None` when `declare` is a name.
    fn declared(&self, k: usize) -> Option<usize> {
        let n = k + 1;
        if self.t.get(n)?.newline {
            return None;
        }
        let e = match self.text(n) {
            "const" | "let" | "var" => {
                let mut j = n + 1;
                loop {
                    j = if self.is(j, "{") || self.is(j, "[") {
                        self.after(j)
                    } else {
                        j + 1
                    };
                    if self.is(j, ":") {
                        j = self.ty(j + 1);
                    }
                    if !self.is(j, ",") {
                        break j;
                    }
                    j += 1;
                }
            }
            "function" => {
                let mut j = n + 1 + usize::from(self.ident(n + 1));
                if self.is(j, "<") {
                    j = self.angles(j, false)?;
                }
                if !self.is(j, "(") {
                    return None;
                }
                j = self.after(j);
                if self.is(j, ":") {
                    j = self.ty(j + 1);
                }
                j
            }
            "type" => return self.types_end(n).ok().flatten(),
            "class" | "enum" | "namespace" | "module" | "global" | "interface" | "abstract" => {
                return Some(self.block_end(n));
            }
            _ => return None,
        };
        Some(e + usize::from(self.is(e, ";")))
    }

    /// One step from token `k`: the index to go on from.
    fn step(&mut self, k: usize) -> Step {
        if self.gone[k] {
            return Ok(k + 1);
        }
        let tok = self.t[k];
        if let Some(o) = self.class_of(k)
            && self.member_start(k, o)
        {
            return self.member(k);
        }
        let text = tok.text(self.src);
        if tok.kind == Kind::Ident && !tok.member && !tok.key {
            if let Some(e) = self.types_end(k)? {
                self.erase(k, e);
                return Ok(e);
            }
            match text {
                "import" if !self.is(k + 1, "(") && !self.is(k + 1, ".") => return self.import(k),
                "export" => return self.export(k),
                "abstract" if self.word(k + 1, "class") => self.erase(k, k + 1),
                "class" => self.class(k),
                "let" | "const" | "var"
                    if self.ident(k + 1) || self.is(k + 1, "{") || self.is(k + 1, "[") =>
                {
                    self.vars(k);
                }
                "as" | "satisfies" if self.operand(k) => {
                    let e = self.ty(k + 1);
                    if e > k + 1 {
                        self.erase(k, e);
                        return Ok(e);
                    }
                }
                _ => {}
            }
        } else if tok.kind == Kind::Punct {
            match text {
                "(" => self.paren(k)?,
                "<" => {
                    // After an operand, type arguments of a call
                    // (`f<T>(x)`, `new Map<K, V>()`, f<T>`…`); else a generic
                    // arrow's parameters, or an assertion (`<T>x`).
                    let operand = self.operand(k);
                    if let Some(e) = self.angles(k, operand) {
                        let call = self.is(e, "(")
                            || self.t.get(e).is_some_and(|n| n.kind == Kind::Template);
                        if !operand || call {
                            self.erase(k, e);
                            return Ok(e);
                        }
                    }
                }
                "!" if !tok.newline && self.operand(k) => self.erase(k, k + 1),
                _ => {}
            }
        }
        Ok(k + 1)
    }

    /// Whether a signature ending before `e` has no body: `;`, the end of
    /// its block, or a new line follows. (Anything else, it misread.)
    fn bodyless(&self, e: usize) -> bool {
        !self.is(e, "{")
            && (self.is(e, ";") || self.is(e, "}") || self.t.get(e).is_none_or(|n| n.newline))
    }

    /// The class body token `k` is directly in.
    fn class_of(&self, k: usize) -> Option<usize> {
        let o = self.open[k];
        self.classes.contains(&o).then_some(o)
    }

    /// Whether `k`, in the class body at `o`, starts a member.
    fn member_start(&self, k: usize, o: usize) -> bool {
        let tok = self.t[k];
        let starts = matches!(tok.kind, Kind::Ident | Kind::String | Kind::Number)
            || matches!(tok.text(self.src), "[" | "*");
        let Some(p) = self.prev(k) else { return false };
        starts
            && (p == o
                || self.is(p, ";")
                || (self.is(p, "}") && self.t[p].depth == tok.depth)
                || (tok.newline && self.ends_operand(p)))
    }

    /// A class member at `k`: its modifiers, `?`, `!`, type parameters and
    /// annotations go; one with no code (`declare`, `abstract`, an
    /// overload, an index signature) goes whole.
    fn member(&mut self, k: usize) -> Step {
        let mut j = k;
        let mut whole = false;
        loop {
            let named = self.t.get(j + 1).is_some_and(|n| {
                !n.newline
                    && (matches!(n.kind, Kind::Ident | Kind::String | Kind::Number)
                        || matches!(n.text(self.src), "[" | "*"))
            });
            if !named || !self.ident(j) {
                break;
            }
            match self.text(j) {
                "public" | "private" | "protected" | "readonly" | "override" => {
                    self.erase(j, j + 1)
                }
                "declare" | "abstract" => whole = true,
                "static" | "async" | "get" | "set" | "accessor" => {}
                _ => break,
            }
            j += 1;
        }
        // A `static { … }` block is code.
        if self.word(j, "static") && self.is(j + 1, "{") {
            return Ok(j + 1);
        }
        j += usize::from(self.is(j, "*"));
        // An index signature: `[key: string]: T`.
        if self.is(j, "[") && self.ident(j + 1) && self.is(j + 2, ":") {
            let mut e = self.after(j);
            if self.is(e, ":") {
                e = self.ty(e + 1);
            }
            e += usize::from(self.is(e, ";"));
            self.erase(k, e);
            return Ok(e);
        }
        let mut m = if self.is(j, "[") {
            self.after(j)
        } else {
            j + 1
        };
        if self.is(m, "?") || self.is(m, "!") {
            self.erase(m, m + 1);
            m += 1;
        }
        if self.is(m, "<")
            && let Some(e) = self.angles(m, false)
        {
            self.erase(m, e);
            m = e;
        }
        if self.is(m, "(") {
            let c = close(&self.t, m);
            self.params(m, c)?;
            let mut e = c + 1;
            if self.is(e, ":") {
                let r = self.ty(e + 1);
                self.erase(e, r);
                e = r;
            }
            if whole || self.bodyless(e) {
                e += usize::from(self.is(e, ";"));
                self.erase(k, e);
                return Ok(e);
            }
            // Its default values are code, as is its body.
            return Ok(m + 1);
        }
        if self.is(m, ":") {
            let e = self.ty(m + 1);
            self.erase(m, e);
            m = e;
        }
        if whole {
            m += usize::from(self.is(m, ";"));
            self.erase(k, m);
        }
        Ok(m)
    }

    /// A `(` at `k`: if it holds parameters (of a function, an arrow, a
    /// method or a `catch`), their types go, and its return type.
    fn paren(&mut self, k: usize) -> Result<(), (usize, String)> {
        let c = close(&self.t, k);
        if c >= self.t.len() {
            return Ok(());
        }
        let p = self.prev(k);
        let pp = p.and_then(|p| self.prev(p));
        let word = |j: Option<usize>, w: &str| j.is_some_and(|j| self.word(j, w));
        // `function f(`, `function* f(`, `function (`.
        let mut f = p;
        if f.is_some_and(|j| self.ident(j) && !self.word(j, "function")) {
            f = f.and_then(|j| self.prev(j));
        }
        if f.is_some_and(|j| self.is(j, "*")) {
            f = f.and_then(|j| self.prev(j));
        }
        let function = f.filter(|&j| self.word(j, "function"));
        let after = c + 1;
        let ret = self.is(after, ":").then(|| self.ty(after + 1));
        let next = ret.unwrap_or(after);
        // `name(…) {` where a method may be (`if (…) {` is no method).
        let named = p.is_some_and(|p| {
            let n = self.t[p];
            matches!(n.kind, Kind::String | Kind::Number)
                || (n.kind == Kind::Ident && !is_reserved(n.text(self.src)))
        });
        let method = named
            && pp.is_some_and(|q| {
                self.is(q, "{")
                    || self.is(q, ",")
                    || self.is(q, "*")
                    || ["get", "set", "async", "static"]
                        .iter()
                        .any(|w| self.word(q, w))
            })
            && self.is(next, "{")
            && !self.t[next].newline;
        let arrow = self.is(next, "=>");
        if !(function.is_some() || method || arrow || word(p, "catch")) {
            return Ok(());
        }
        self.params(k, c)?;
        if let Some(r) = ret {
            self.erase(after, r);
        }
        // A function with no body: an overload's signature.
        if let Some(f) = function
            && self.bodyless(next)
        {
            let mut s = f;
            for w in ["async", "default", "export"] {
                if let Some(q) = self.prev(s).filter(|&q| self.word(q, w)) {
                    s = q;
                }
            }
            self.erase(s, next + usize::from(self.is(next, ";")));
        }
        Ok(())
    }

    /// The parameters in `(…)` from `k` to `c`: `?`, annotations and a
    /// `this` parameter go; a parameter property is an error.
    fn params(&mut self, k: usize, c: usize) -> Result<(), (usize, String)> {
        let d = self.t[k].depth + 1;
        let mut j = k + 1;
        while j < c {
            let property = ["public", "private", "protected", "readonly", "override"]
                .iter()
                .any(|w| self.word(j, w))
                && (self.ident(j + 1) || self.is(j + 1, "{") || self.is(j + 1, "["));
            if property {
                return Err((self.t[j].start, PARAMETER_PROPERTY.into()));
            }
            if self.word(j, "this") && self.is(j + 1, ":") {
                let e = self.ty(j + 2);
                let e = e + usize::from(self.is(e, ","));
                self.erase(j, e);
                j = e;
                continue;
            }
            j += usize::from(self.is(j, "..."));
            let mut m = if self.is(j, "{") || self.is(j, "[") {
                self.after(j)
            } else {
                j + 1
            };
            if self.is(m, "?") {
                self.erase(m, m + 1);
                m += 1;
            }
            if self.is(m, ":") {
                let e = self.ty(m + 1);
                self.erase(m, e);
                m = e;
            }
            // A default value is code: on to the next parameter.
            while m < c && !(self.t[m].depth == d && self.is(m, ",")) {
                m += 1;
            }
            j = m + 1;
        }
        Ok(())
    }

    /// `let`, `const` or `var` at `k`: each name's `!` and annotation go.
    fn vars(&mut self, k: usize) {
        let d = self.t[k].depth;
        let mut j = k + 1;
        loop {
            let mut m = if self.is(j, "{") || self.is(j, "[") {
                self.after(j)
            } else if self.ident(j) {
                j + 1
            } else {
                return;
            };
            if self.is(m, "!") {
                self.erase(m, m + 1);
                m += 1;
            }
            if self.is(m, ":") {
                let e = self.ty(m + 1);
                self.erase(m, e);
                m = e;
            }
            let e = if self.is(m, "=") {
                self.value_end(m + 1, d)
            } else {
                m
            };
            if !(self.is(e, ",") && self.t[e].depth == d) {
                return;
            }
            j = e + 1;
        }
    }

    /// Where the value at `s`, in a list at depth `d`, ends: at a `,` or
    /// `;`, its bracket's end, or a line that starts another statement.
    fn value_end(&self, s: usize, d: u32) -> usize {
        let mut j = s;
        while let Some(n) = self.t.get(j) {
            if n.depth < d {
                break;
            }
            if n.depth == d && j > s {
                let text = n.text(self.src);
                if matches!(text, "," | ";")
                    || (n.newline && n.kind != Kind::Punct && self.ends_operand(j - 1))
                {
                    break;
                }
                // `f<A, B>(x)`: its comma is not the list's.
                if text == "<"
                    && self.ends_operand(j - 1)
                    && let Some(e) = self.angles(j, true)
                    && self.is(e, "(")
                {
                    j = e;
                    continue;
                }
            }
            j += 1;
        }
        j
    }

    /// `class` at `k`: its type parameters, its base's type arguments and
    /// `implements …` go, and its body is a class body.
    fn class(&mut self, k: usize) {
        let d = self.t[k].depth;
        let mut j = k + 1;
        if self.ident(j) && !self.word(j, "extends") && !self.word(j, "implements") {
            j += 1;
        }
        if self.is(j, "<")
            && let Some(e) = self.angles(j, false)
        {
            self.erase(j, e);
            j = e;
        }
        while let Some(n) = self.t.get(j) {
            if n.depth < d {
                return;
            }
            if n.depth == d {
                if self.is(j, "{") {
                    self.classes.push(j);
                    return;
                }
                if self.word(j, "implements") {
                    let b = (j..self.t.len())
                        .find(|&i| self.t[i].depth == d && self.is(i, "{"))
                        .unwrap_or(self.t.len());
                    self.erase(j, b);
                    j = b;
                    continue;
                }
                if self.is(j, "<")
                    && let Some(e) = self.angles(j, true)
                    && (self.is(e, "{") || self.word(e, "implements"))
                {
                    self.erase(j, e);
                    j = e;
                    continue;
                }
            }
            j += 1;
        }
    }

    /// `import` at `k`: `import type …` goes whole, a `type` specifier
    /// alone. The index after the statement.
    fn import(&mut self, k: usize) -> Step {
        if self.ident(k + 1) && self.is(k + 2, "=") {
            return Err((
                self.t[k].start,
                "`import x = …` is not erasable TypeScript (it makes code): use `import x from '…'`".into(),
            ));
        }
        let d = self.t[k].depth;
        let Some(s) =
            (k + 1..self.t.len()).find(|&j| self.t[j].kind == Kind::String && self.t[j].depth == d)
        else {
            return Ok(k + 1);
        };
        let end = s + 1 + usize::from(self.is(s + 1, ";"));
        if self.word(k + 1, "type") && !self.is(k + 2, ",") && !self.word(k + 2, "from") {
            self.erase(k, end);
        } else if let Some(o) = (k + 1..s).find(|&j| self.is(j, "{")) {
            self.specifiers(o);
        }
        Ok(end)
    }

    /// `export` at `k`.
    fn export(&mut self, k: usize) -> Step {
        let n = k + 1;
        if self.is(n, "=") {
            return Err((
                self.t[k].start,
                "`export =` is not erasable TypeScript (it makes code): use `export default`"
                    .into(),
            ));
        }
        let typed = self.word(n, "type") && (self.is(n + 1, "{") || self.is(n + 1, "*"));
        if typed || self.is(n, "{") || self.is(n, "*") {
            let at = n + usize::from(typed);
            if self.is(at, "{") {
                self.specifiers(at);
            }
            let mut e = if self.is(at, "{") {
                self.after(at)
            } else {
                at + 1
            };
            if self.word(e, "as") {
                e += 2;
            }
            if self.word(e, "from") {
                e += 2;
            }
            e += usize::from(self.is(e, ";"));
            if typed {
                self.erase(k, e);
            }
            return Ok(e);
        }
        let at = n + usize::from(self.word(n, "default"));
        if self.t.get(at).is_some()
            && let Some(e) = self.types_end(at)?
        {
            self.erase(k, e);
            return Ok(e);
        }
        Ok(n)
    }

    /// `type A` (or `type A as B`) in the `{ … }` at `o` of an import or
    /// export: gone, with its comma.
    fn specifiers(&mut self, o: usize) {
        let c = close(&self.t, o);
        let d = self.t[o].depth + 1;
        let mut j = o + 1;
        while j < c {
            let e = (j..c)
                .find(|&i| self.t[i].depth == d && self.is(i, ","))
                .unwrap_or(c);
            // `{ type as }` imports `as`'s type; `{ type as x }` the value `type`.
            if self.word(j, "type") && matches!(e - j, 2 | 4) {
                self.erase(j, (e + 1).min(c));
            }
            j = e + 1;
        }
    }
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

    /// `src` stripped, its whitespace runs made one space: and checks that
    /// every line and column stayed where it was.
    fn strip(src: &str) -> String {
        let out = strip_types(src).unwrap_or_else(|e| panic!("{e:?} in {src}"));
        assert_eq!(out.len(), src.len(), "{out}");
        let lines = |s: &str| s.match_indices('\n').map(|(i, _)| i).collect::<Vec<_>>();
        assert_eq!(lines(&out), lines(src));
        out
    }

    /// Whether `out` is `expected` but for whitespace, other than between
    /// two words.
    fn same(out: String, expected: &str) {
        let norm = |s: &str| {
            let mut n = String::new();
            let mut gap = false;
            for c in s.chars() {
                if c.is_whitespace() {
                    gap = true;
                    continue;
                }
                let word = |c: char| c.is_alphanumeric() || c == '_' || c == '$';
                if gap && n.chars().last().is_some_and(word) && word(c) {
                    n.push(' ');
                }
                gap = false;
                n.push(c);
            }
            n
        };
        assert_eq!(norm(&out), norm(expected), "{out}");
    }

    fn strip_err(src: &str) -> String {
        strip_types(src).expect_err(src).1
    }

    #[test]
    fn types_go_annotations() {
        same(
            strip("let x: number = 1, y: Array<string[]> = [], z!: Map<string, Set<number>>"),
            "let x = 1, y = [], z",
        );
        same(
            strip(
                "function f<T extends object = {}>(a: T, b?: number, ...rest: string[]): T[] { return [a] }",
            ),
            "function f(a, b, ...rest) { return [a] }",
        );
        same(
            strip("const h = (a: number, { b }: { b: string } = { b: '' }): void => {}"),
            "const h = (a, { b } = { b: '' }) => {}",
        );
        same(
            strip("const g = async <T,>(x: T): Promise<T> => x"),
            "const g = async (x) => x",
        );
        same(strip("const k = <T>(x: T) => x"), "const k = (x) => x");
        same(
            strip("items.map((i: Item): string => i.name).filter((s): s is string => !!s)"),
            "items.map((i) => i.name).filter((s) => !!s)",
        );
        same(
            strip("let cb: (a: number) => void = () => {}, u: { a: 1 } | null = null"),
            "let cb = () => {}, u = null",
        );
        same(
            strip(
                "try { f() } catch (e: unknown) { g(e) }\nfunction m(this: Window, a: number) {}",
            ),
            "try { f() } catch (e) { g(e) } function m(a) {}",
        );
        same(
            strip(
                "for (const [k, v] of map) { let n: number = k }\nfor (let i: number = 0; i < n; i++) {}",
            ),
            "for (const [k, v] of map) { let n = k } for (let i = 0; i < n; i++) {}",
        );
    }

    #[test]
    fn types_go_generics_but_comparisons_stay() {
        same(strip("const c = a < b > c"), "const c = a < b > c");
        same(
            strip("if (a < b && c > d) { go() }"),
            "if (a < b && c > d) { go() }",
        );
        same(strip("x = a < b; y = c > (d)"), "x = a < b; y = c > (d)");
        same(strip("const s = a<b || c>(d)"), "const s = a<b || c>(d)");
        same(
            strip("const r = n >> 2, q = n >>> 1, t = n >= 3"),
            "const r = n >> 2, q = n >>> 1, t = n >= 3",
        );
        same(strip("const d = f<string>(x)"), "const d = f(x)");
        same(
            strip("let m = new Map<string, Set<number>>(), n: number"),
            "let m = new Map(), n",
        );
        same(
            strip("const q = sql<Row>`select 1`"),
            "const q = sql`select 1`",
        );
        same(strip("this.emit<Event>('x', e)"), "this.emit('x', e)");
    }

    #[test]
    fn types_go_casts_and_non_null() {
        same(
            strip("const o = { a: 1, b: [2] } as const"),
            "const o = { a: 1, b: [2] }",
        );
        same(strip("(x as unknown as T).y = z satisfies Z"), "(x).y = z");
        same(strip("f(x as Foo<Bar>[], y)"), "f(x, y)");
        same(
            strip("const el = document.querySelector('a')!; el!.click(); a![0]!.b"),
            "const el = document.querySelector('a'); el.click(); a[0].b",
        );
        same(
            strip("if (x) !y && z()\nlet a = !b"),
            "if (x) !y && z() let a = !b",
        );
        same(
            strip("const t = `${(n as number) + 1}px: ${x as string}`"),
            "const t = `${(n) + 1}px: ${x}`",
        );
        same(
            strip("const r = /a<b>c: d/g; const ok = r.test(s as string) ? 1 : 2"),
            "const r = /a<b>c: d/g; const ok = r.test(s) ? 1 : 2",
        );
    }

    #[test]
    fn types_go_declarations() {
        let src = "interface P { a: string }\ntype Q<T> = T | null;\nexport type R = { x: number }\n\
                   export interface S extends P {}\ntype U =\n  | 'a'\n  | 'b'\nlet z = 1\n\
                   type M<T> = { [K in keyof T]?: T[K] extends string ? K : never }\n\
                   declare const API: string;\ndeclare function g(x: number): void\n\
                   declare module 'x' { export const y: number }\ndeclare global { interface Window { a: number } }\n\
                   namespace T { export type X = number; interface Y {} }\nexport default 1";
        same(strip(src), "let z = 1 export default 1");
        same(
            strip(
                "import type { A } from './a'\nimport { type B, c, type D as E } from './b';\nimport type from './t'\n\
                   export type { A }\nexport { type B as BB, c }\nexport type * from './x'",
            ),
            "import { c, } from './b'; import type from './t' export { c }",
        );
        same(
            strip(
                "function f(a: string): string;\nfunction f(a: number): number\nfunction f(a: any) { return a }",
            ),
            "function f(a) { return a }",
        );
        same(
            strip("export function f(): void;\nexport function f() {}"),
            "export function f() {}",
        );
        // Plain JavaScript stays as it is.
        let js = "let a = b ? c : d; const o = { a: 1, b: c ? 1 : 2, type: 3, m(x) { return x } };\n\
                  label: for (;;) { break label }\nswitch (x) { case 1: f(); default: g() }\nlet type = 1, as = 2; type = as";
        same(strip(js), js);
    }

    #[test]
    fn types_go_classes() {
        let src = "abstract class A<T> extends B<T> implements C, D<T> {\n  private readonly x: number = 1\n  declare y: string\n  \
                   static z?: T\n  #w!: boolean\n  [key: string]: unknown\n  constructor(a: number) { super(); this.x = a }\n  \
                   abstract m(): void\n  get v(): number { return this.x }\n  n<U>(u: U): U { return u }\n  \
                   over(a: string): void;\n  over(a: any) {}\n  public async *gen(): AsyncGenerator<number> {}\n  static { init() }\n}";
        same(
            strip(src),
            "class A extends B { x = 1 static z #w constructor(a) { super(); this.x = a } get v() { return this.x } \
             n(u) { return u } over(a) {} async *gen() {} static { init() } }",
        );
        same(
            strip(
                "const o = { m(a: number): string { return '' }, get x(): number { return 1 }, async n<T>(t?: T) {} }",
            ),
            "const o = { m(a) { return '' }, get x() { return 1 }, async n(t) {} }",
        );
        same(
            strip("class P { handle = (e: Event): void => { this.x = e }; private n = 0 }"),
            "class P { handle = (e) => { this.x = e }; n = 0 }",
        );
    }

    #[test]
    fn types_go_in_tricky_places() {
        same(
            strip("let y: typeof x = x, k: keyof typeof o = 'a', a = <T>b, s: unique symbol"),
            "let y = x, k = 'a', a = b, s",
        );
        same(
            strip(
                "export const id = <T,>(x: T): T => x\nconst v = c ? (a as A) : (b as B)\nconst w = c ? (a) : b",
            ),
            "export const id = (x) => x const v = c ? (a) : (b) const w = c ? (a) : b",
        );
        same(
            strip(
                "class Q<T = string> { private constructor(a: T) {} protected static make(): Q<number> { return new Q(1) } }",
            ),
            "class Q { constructor(a) {} static make() { return new Q(1) } }",
        );
        same(
            strip("let f = a?.b!.c, g = (h as any)?.[0]"),
            "let f = a?.b.c, g = (h)?.[0]",
        );
        same(
            strip("const p = new Promise<void>((resolve: () => void) => setTimeout(resolve, 1))"),
            "const p = new Promise((resolve) => setTimeout(resolve, 1))",
        );
        same(
            strip("function g(): { a: number; b(): void } {\n  return { a: 1, b() {} }\n}"),
            "function g() { return { a: 1, b() {} } }",
        );
    }

    #[test]
    fn public_env_is_filled_in() {
        let vars = |n: &str| match n {
            "PUBLIC_API" => Some("https://x.io/\"a\"".to_string()),
            "PUBLIC_EMPTY" => Some(String::new()),
            _ => None,
        };
        let fill = |src: &str| public_env(src, &vars);
        assert_eq!(
            fill("fetch(env.PUBLIC_API + '/a')\nlet e = env.PUBLIC_EMPTY || x.env.y"),
            Ok("fetch(\"https://x.io/\\\"a\\\"\" + '/a')\nlet e = \"\" || x.env.y".into())
        );
        // An `env` of the code's own is its own.
        for own in [
            "let env = {}; env.SECRET",
            "import { env } from '$lib/e.js'\nenv.KEY",
            "function f(env) { return env.SECRET }",
            "const o = { env: 1 }",
        ] {
            assert_eq!(fill(own), Ok(own.into()), "{own}");
        }
        let err = |src: &str| fill(src).unwrap_err();
        let (at, msg) = err("let a = 1\nlet k = env.SECRET_KEY");
        assert_eq!(at, 18);
        assert!(
            msg.contains("`env.SECRET_KEY` is not sent to the browser")
                && msg.contains("wisp::env(\"SECRET_KEY\")"),
            "{msg}"
        );
        assert!(
            err("env.PUBLIC_MISSING")
                .1
                .contains("`env.PUBLIC_MISSING` is not set")
        );
        assert!(err("console.log(env)").1.contains("a name at a time"));
        assert!(err("env['PUBLIC_API']").1.contains("a name at a time"));
    }

    #[test]
    fn types_that_make_code_are_errors() {
        assert!(strip_err("enum E { A, B }").contains("`enum` is not erasable"));
        assert!(strip_err("const enum E { A }").contains("as const"));
        assert!(strip_err("export enum E { A }").contains("`enum`"));
        assert!(strip_err("namespace N { export const x = 1 }").contains("namespace"));
        assert!(
            strip_err("class A { constructor(private x: number) {} }")
                .contains("parameter property")
        );
        assert!(strip_err("import fs = require('fs')").contains("import x from"));
        assert!(strip_err("export = x").contains("export default"));
        // At the offset of what is wrong.
        assert_eq!(strip_types("let a = 1\nenum E {}").unwrap_err().0, 10);
    }

    #[test]
    fn runes_and_state() {
        let data = ["data".to_string()];
        let src = "import { cart } from '$lib/cart.js'\nlet n = 0, m\nconst k = 1\nlet double = $derived(n * 2)\n\
                   let big = $derived.by(() => {\n  return n > 9\n})\nlet list = $state([1])\nconst raw = $state.raw({ a: 1 })\n\
                   function inc(k) { n++; list.push(n); return { n, k, m: data.x } }\n$effect(() => console.log(double, $cart))\n\
                   $effect.pre(() => {})\nconst plain = $state.snapshot(list)\nlet { a, b: c } = data.y\n$inspect(n, list)";
        let (out, r) = script(src, &data, false, &[]).unwrap();
        assert_eq!(
            out,
            "import { cart } from '$lib/cart.js'\nlet n = __wisp_s(0), m = __wisp_s()\nconst k = 1\nlet double = __wisp_d(() => (n.v * 2))\n\
             let big = __wisp_d(() => {\n  return n.v > 9\n})\nlet list = __wisp_s([1])\nconst raw = __wisp_r({ a: 1 })\n\
             function inc(k) { n.v++; list.v.push(n.v); return { n: n.v, k, m: data.v.x } }\n__wisp_e(() => console.log(double.v, cart.value))\n\
             __wisp_ep(() => {})\nconst plain = __wisp_snap(list.v)\nlet { a: __wisp_a, b: __wisp_c } = data.v.y, a = __wisp_s(__wisp_a), c = __wisp_s(__wisp_c)\n\
             __wisp_e(() => console.log(...[n.v, list.v].map(__wisp_snap)))"
        );
        assert_eq!(
            r.state,
            ["data", "n", "m", "double", "big", "list", "raw", "a", "c"]
        );
        assert_eq!(r.derived, ["double", "big"]);
        // Release drops `$inspect`, keeping the lines.
        let (out, _) = script("let n = 1\n$inspect(\n  n\n)\nn++", &[], true, &[]).unwrap();
        assert_eq!(out, "let n = __wisp_s(1)\nvoid 0\n\n\nn.v++");
        // Set to a literal and never again, here or in `extra`: a constant.
        let (out, r2) = script(
            "let a = 1, b = 'x', c = 2\nf(a, b, c)",
            &[],
            false,
            &["(_, v) => { c = v }"],
        )
        .unwrap();
        assert_eq!(out, "let a = 1, b = 'x', c = __wisp_s(2)\nf(a, b, c.v)");
        assert_eq!(r2.state, ["c"]);
        // State fields of a class.
        let (out, _) = script(
            "class T {\n  done = $state(false)\n  n = $derived(this.done ? 1 : 0);\n  go() { this.done = true }\n}",
            &[],
            false,
            &[],
        )
        .unwrap();
        assert_eq!(
            out,
            "class T {\n  #w_done = __wisp_s(false); get done() { return this.#w_done.v } set done(v) { this.#w_done.v = v }\n  \
             #w_n = __wisp_d(() => (this.done ? 1 : 0)); get n() { return this.#w_n.v };\n  go() { this.done = true }\n}"
        );
        // A block is no object: `{ n = 2 }` in a body assigns.
        let (out, _) = script(
            "let n = 1\nfunction f() { n = 2 }\nconst o = { n }",
            &[],
            false,
            &[],
        )
        .unwrap();
        assert_eq!(
            out,
            "let n = __wisp_s(1)\nfunction f() { n.v = 2 }\nconst o = { n: n.v }"
        );
        // Directives read the same names; their own parameters shadow them.
        assert_eq!(
            rewrite(
                "({ item }, event) => (n++, item.x + double + $cart.length)",
                &r
            )
            .unwrap(),
            "({ item }, event) => (n.v++, item.x + double.v + cart.value.length)"
        );
        // `state === e` is a selector; not where it is an operand of
        // something tighter, on a derived value, or split over lines.
        for (src, out) in [
            (
                "({ row }) => (n === row.id)",
                "({ row }) => (__wisp_eq(n, row.id))",
            ),
            (
                "({ row }) => (n !== row.id && list)",
                "({ row }) => (!__wisp_eq(n, row.id) && list.v)",
            ),
            (
                "() => (a ? n === f(m) : 0)",
                "() => (a.v ? __wisp_eq(n, f(m.v)) : 0)",
            ),
            ("() => (!n === 1)", "() => (!n.v === 1)"),
            ("() => (n.x === 1)", "() => (n.v.x === 1)"),
            ("() => (double === 1)", "() => (double.v === 1)"),
            ("() => { n === 1\nf() }", "() => { __wisp_eq(n, 1)\nf() }"),
            ("() => { n === a\n.b }", "() => { n.v === a.v\n.b }"),
        ] {
            assert_eq!(rewrite(src, &r).unwrap(), out);
        }
        for (bad, what) in [
            ("f($derived(1))", "$derived"),
            ("let x = $derived()", "$derived"),
            ("{ let y = $state(1) }", "$state"),
            ("let { a } = $state(x)", "$state"),
            ("let d = $derived(1)\nd = 2", "d"),
            ("let d = $derived(1)\nfunction f() { d++ }", "d"),
            ("$effect.root(() => {})", "$effect"),
            ("$inspect(x).with(f)", "$inspect"),
            ("$host()", "$host"),
            ("let { a } = $props()", "$props"),
        ] {
            let (off, msg) = script(bad, &[], false, &[]).unwrap_err();
            assert!(
                bad[off..].starts_with(what) && msg.contains('`'),
                "{bad}: {msg}"
            );
        }
        assert!(rewrite("$state(1)", &r).unwrap_err().1.contains("script"));
    }

    #[test]
    fn props_rune_and_first_paint() {
        let src = "const x = 1\nlet { title, count = 0, open = $bindable(), n = $bindable(f(1, 2)) } = $props();\nlet y";
        let p = props_rune(src).unwrap().unwrap();
        assert_eq!(
            &src[p.span.0..p.span.1],
            "let { title, count = 0, open = $bindable(), n = $bindable(f(1, 2)) } = $props();"
        );
        let got: Vec<(&str, Option<&str>, bool)> = p
            .props
            .iter()
            .map(|x| (x.name.as_str(), x.default.as_deref(), x.bindable))
            .collect();
        assert_eq!(
            got,
            [
                ("title", None, false),
                ("count", Some("0"), false),
                ("open", None, true),
                ("n", Some("f(1, 2)"), true),
            ]
        );
        // Renamed, and the rest.
        let p = props_rune("let { class: cls = 'x', ...rest } = $props()")
            .unwrap()
            .unwrap();
        assert_eq!(
            (p.props[0].name.as_str(), p.props[0].local.as_str()),
            ("class", "cls")
        );
        assert_eq!(p.rest.as_deref(), Some("rest"));
        assert_eq!(props_rune("let x = 1").unwrap(), None);
        for bad in [
            "const p = $props()",
            "let { ...rest, a } = $props()",
            "let { a: 1 } = $props()",
            "let { a, a } = $props()",
            "f($props())",
        ] {
            assert!(props_rune(bad).is_err(), "{bad}");
        }
        assert_eq!(plain_init("$state([1, 2])"), Some("[1, 2]"));
        assert_eq!(plain_init("$derived(a)"), None);
        assert_eq!(plain_init("$state()"), None);
        assert_eq!(plain_init("data.x"), Some("data.x"));
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

    use wisp_shared::rng::Rng;

    const PIECES: &[&str] = &[
        "let ",
        "const ",
        "var ",
        "x",
        "y",
        "data",
        ".",
        "?.",
        "(",
        ")",
        "[",
        "]",
        "{",
        "}",
        ",",
        ";",
        ":",
        "=",
        "=>",
        "+",
        "++",
        "-",
        "/",
        "*",
        "!",
        "?",
        " ",
        "\n",
        "'",
        "\"",
        "`",
        "${",
        "\\",
        "/*",
        "*/",
        "//",
        "1",
        "0x1f",
        "1e-3",
        "$state(",
        "$derived(",
        "$derived.by(",
        "$effect(",
        "$props()",
        "$bindable(",
        "$inspect(",
        "$cart",
        "...",
        "class ",
        "function ",
        "return ",
        "import ",
        "from ",
        "export ",
        "é",
        "😀",
        "#p",
        "this",
        "new ",
        "async ",
        "in ",
        "of ",
        "for ",
        "if ",
        "else ",
        "catch ",
        "get ",
        "static ",
        "=",
        "a = $state(0)",
        "{ a, b: c = 1, ...r }",
        "/re/g",
        "`a${b}c`",
    ];

    /// Random scripts, well formed or not: the tokenizer and everything
    /// built on it never panic, and minified code tokenizes the same.
    #[test]
    fn fuzz_never_panics() {
        let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
        let r = Reactive {
            state: vec!["x".into(), "data".into()],
            derived: vec!["y".into()],
            stores: vec!["cart".into()],
        };
        for round in 0..4000 {
            let src = rng.pieces(PIECES, 1 + round % 40);
            let t = tokens(&src);
            assert!(
                t.iter().all(|k| k.start < k.end && k.end <= src.len()),
                "{src:?}"
            );
            let _ = chains(&src);
            let _ = declarations(&src);
            let _ = imports(&src);
            let _ = props_rune(&src);
            let _ = script(&src, &r.state, round % 2 == 0, &["x = 1"]);
            let _ = rewrite(&src, &r);
            let _ = (initializer(&src, "x"), plain_init(&src), each(&src));
            let _ = (
                is_path(&src),
                is_statements(&src),
                ends_in_line_comment(&src),
            );
            let min = minify(&src);
            // Unterminated strings end at a line break, which minifying can
            // take away; elsewhere the tokens are the same.
            if !src.contains(['\'', '"', '`', '\\']) {
                let a: Vec<&str> = t.iter().map(|k| k.text(&src)).collect();
                let b: Vec<&str> = tokens(&min).iter().map(|k| k.text(&min)).collect();
                assert_eq!(a, b, "{src:?} -> {min:?}");
            }
        }
    }

    #[test]
    fn minify_keeps_statements_apart() {
        assert_eq!(
            minify(
                "// c\nlet a = 1\nlet b = a\n/* x */ return\nb\nx = y\n++z\nf(1) / 2\nq = /re/ in o"
            ),
            "let a=1\nlet b=a\nreturn\nb\nx=y\n++z\nf(1)/2\nq=/re/ in o"
        );
        assert_eq!(
            minify("a + +b - -c; 1 .x; `a ${ b } c`"),
            "a+ +b- -c;1 .x;`a ${b} c`"
        );
        // The runtime's extra half, as release builds serve it.
        let src = wisp_shared::EXTRA_JS;
        let texts =
            |s: &str| -> Vec<String> { tokens(s).iter().map(|t| t.text(s).to_string()).collect() };
        assert_eq!(texts(src), texts(&minify(src)));
    }

    #[test]
    fn mangle_renames_what_is_bound() {
        let src = "import { page } from 'wisp'\nexport const kept = 1\nconst observer = 2\n\
                   function update(node, { deep }) { const node2 = { node, deep: 1 }; return node2.node + observer }\n\
                   const o = { update(x) { return x }, get size() { return observer } }\n\
                   try { f() } catch (error) { console.log(error, document) }\n\
                   switch (observer) { case 1: { observer; break } }";
        // The most used names get the shortest, never a global's (`f`);
        // globals, imports, exports, keys, properties and method names stay.
        assert_eq!(
            mangle(src),
            "import { page } from 'wisp'\nexport let kept = 1\nlet a = 2\n\
             function i(c, { deep: g }) { let d = { node: c, deep: 1 }; return d.node + a }\n\
             let h = { update(e) { return e }, get size() { return a } }\n\
             try { f() } catch (b) { console.log(b, document) }\n\
             switch (a) { case 1: { a; break } }"
        );
        // `export { … }` leaves a file as it is.
        let src = "const x = 1\nexport { x }";
        assert_eq!(mangle(src), src);
    }
}
