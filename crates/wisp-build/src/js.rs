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

/// The variables `src` reads, each with the properties read from it:
/// `data.user.name` is `["data", "user", "name"]`. A chain stops before a
/// call or an index (`key.mark.class()` is `["key", "mark"]`), and at `?.`.
/// Each comes with the offset where it starts. Properties, object keys and
/// reserved words are not variables; names declared inside functions are
/// not told apart from outer ones.
pub fn chains(src: &str) -> Vec<(Vec<String>, usize)> {
    let t = tokens(src);
    let mut out = Vec::new();
    let mut k = 0;
    while k < t.len() {
        let tok = t[k];
        let word = tok.text(src);
        if tok.kind != Kind::Ident
            || tok.member
            || tok.key
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
                let Some(name) = ident(k + 1) else {
                    k += 1;
                    continue;
                };
                out.push((name.text(src).to_string(), name.start));
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
                        if text == ","
                            && let Some(next) = ident(j + 1)
                        {
                            out.push((next.text(src).to_string(), next.start));
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
        let src = "let a = 1, b = f(x, y)\nconst c = { d: 1 }\nfunction e() { let inner }\nclass F {}\nasync function* g() {}\nvar h\nfoo, bar\nif (x) { let no }\nconst k = function named() {}";
        let names: Vec<String> = declarations(src).into_iter().map(|(n, _)| n).collect();
        assert_eq!(names, ["a", "b", "c", "e", "F", "g", "h", "k"]);
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
