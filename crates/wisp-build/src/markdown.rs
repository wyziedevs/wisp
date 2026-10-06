//! Markdown pages: `+page.md`, and each `src/routes/<dir>/x.md`. A page is
//! turned into `.wisp` markup at build time, so it compiles (and bakes,
//! having no Rust) like any other page.
//!
//! ```text
//! ---
//! title: Hello
//! layout: Post          (a component: the page is its children)
//! date: 2026-10-01      (any other field: data, and a prop of the layout)
//! ---
//! # Hello
//! <Card title="x">
//!
//! Markdown in a component, between blank lines.
//!
//! </Card>
//! ```
//!
//! Text and code are written with `{`/`}` as character references, so no
//! hole comes from them; raw HTML (components) is the template's own.

use crate::codegen::Comp;
use pulldown_cmark::{CodeBlockKind, CowStr, Event, LinkType, Options, Parser, Tag, TagEnd};

/// A Markdown page made ready to compile.
#[derive(Debug)]
pub struct Md {
    /// The page as `.wisp` markup.
    pub wisp: String,
    /// The front matter, in its order, `title` added from the first `# h1`
    /// when it has none: what `wisp::pages` lists.
    pub fields: Vec<(String, String)>,
}

/// The fields of the front matter and the line the body starts on (0
/// without front matter). Errors are `line: msg`.
pub fn front(src: &str) -> Result<(Vec<(String, String)>, usize), String> {
    let mut lines = src.split('\n');
    if lines.next().map(str::trim_end) != Some("---") {
        return Ok((Vec::new(), 0));
    }
    let mut fields: Vec<(String, String)> = Vec::new();
    for (k, line) in lines.enumerate() {
        let n = k + 2;
        let line = line.trim_end();
        if line == "---" {
            return Ok((fields, n));
        }
        if line.trim().is_empty() || line.trim_start().starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once(':') else {
            return Err(format!("{n}: front matter lines are `name: value`"));
        };
        let key = key.trim();
        if !crate::ty::is_ident(key) {
            return Err(format!(
                "{n}: `{key}` is not a field name (letters, digits and `_`)"
            ));
        }
        if fields.iter().any(|(k, _)| k == key) {
            return Err(format!("{n}: `{key}` is set twice"));
        }
        let v = value.trim();
        let v = match (v.as_bytes().first(), v.as_bytes().last()) {
            (Some(b'"'), Some(b'"')) | (Some(b'\''), Some(b'\'')) if v.len() > 1 => {
                &v[1..v.len() - 1]
            }
            _ => v,
        };
        fields.push((key.to_string(), v.to_string()));
    }
    Err("1: this `---` starts the front matter, which needs a `---` line after it".into())
}

/// The page at `src` as markup: its `<title>` (none when `layout_title`:
/// a layout above writes the one title), its body in its `layout`
/// component (one of `comps`) given the fields it takes as props. Errors
/// are `line: msg`.
pub fn page(src: &str, comps: &[Comp], layout_title: bool) -> Result<Md, String> {
    let (mut fields, start) = front(src)?;
    let body: String = src.split('\n').skip(start).collect::<Vec<_>>().join("\n");
    let (html, h1) = render(&body);
    if !fields.iter().any(|(k, _)| k == "title")
        && let Some(h1) = h1
    {
        fields.push(("title".into(), h1));
    }
    let get = |k: &str| fields.iter().find(|(n, _)| n == k).map(|(_, v)| v.as_str());
    let noindex = match get("noindex") {
        None | Some("false") => false,
        Some("true") => true,
        Some(v) => return Err(format!("1: `noindex: {v}`: write true or false")),
    };
    let mut out = String::new();
    let title = get("title").filter(|_| !layout_title);
    if title.is_some() || noindex {
        out.push_str("<head>");
        if let Some(t) = title {
            out.push_str(&format!("<title>{}</title>", text(t)));
        }
        if noindex {
            out.push_str("<meta name=\"robots\" content=\"noindex\">");
        }
        out.push_str("</head>\n");
    }
    let Some(layout) = get("layout") else {
        out.push_str(&html);
        return Ok(Md { wisp: out, fields });
    };
    let Some(c) = comps.iter().find(|c| c.name == layout) else {
        return Err(format!(
            "1: `layout: {layout}` names a component: add src/components/{layout}.wisp"
        ));
    };
    if !c.children {
        return Err(format!(
            "1: layout {layout} ({}) shows no page: give it {{@render children()}}",
            c.rel
        ));
    }
    out.push('<');
    out.push_str(layout);
    for p in &c.props {
        let Some(v) = get(&p.name) else {
            if p.default.is_none() && !p.ty.starts_with("Option<") {
                return Err(format!(
                    "1: layout {layout} needs `{}` in the front matter",
                    p.name
                ));
            }
            continue;
        };
        let value = prop(&p.ty, v).ok_or_else(|| {
            format!(
                "1: `{}: {v}` cannot be {layout}'s `{}: {}` (front matter gives &str, String, bool, numbers, or Option of one)",
                p.name, p.name, p.ty
            )
        })?;
        out.push_str(&format!(" {}={{{value}}}", p.name));
    }
    out.push_str(">\n");
    out.push_str(&html);
    out.push_str(&format!("</{layout}>\n"));
    Ok(Md { wisp: out, fields })
}

/// A front matter value as the Rust a prop of type `ty` takes.
fn prop(ty: &str, v: &str) -> Option<String> {
    if let Some(inner) = ty.strip_prefix("Option<").and_then(|t| t.strip_suffix('>')) {
        return prop(inner, v).map(|x| format!("Some({x})"));
    }
    match ty.trim() {
        "&str" | "&'static str" | "String" => Some(format!("{v:?}")),
        "bool" => matches!(v, "true" | "false").then(|| v.to_string()),
        t => {
            // A number of that type: `3_u32`, `1.5_f64`.
            let digits = v
                .trim_start_matches('-')
                .starts_with(|c: char| c.is_ascii_digit());
            let fits = match t {
                "f32" | "f64" => v.parse::<f64>().is_ok(),
                "u8" => v.parse::<u8>().is_ok(),
                "u16" => v.parse::<u16>().is_ok(),
                "u32" => v.parse::<u32>().is_ok(),
                "u64" => v.parse::<u64>().is_ok(),
                "usize" => v.parse::<usize>().is_ok(),
                "i8" => v.parse::<i8>().is_ok(),
                "i16" => v.parse::<i16>().is_ok(),
                "i32" => v.parse::<i32>().is_ok(),
                "i64" => v.parse::<i64>().is_ok(),
                "isize" => v.parse::<isize>().is_ok(),
                _ => false,
            };
            (digits && fits).then(|| format!("{v}_{t}"))
        }
    }
}

/// Text as markup: escaped, with braces as character references.
fn text(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '{' => out.push_str("&#123;"),
            '}' => out.push_str("&#125;"),
            _ => out.push(c),
        }
    }
    out
}

/// A URL for an attribute: escaped, braces percent-encoded.
fn url(s: &str) -> String {
    text(&no_braces(s))
}

/// `s` with its braces percent-encoded.
fn no_braces(s: &str) -> String {
    s.replace('{', "%7B").replace('}', "%7D")
}

/// The body as HTML, and the text of its first `# h1`.
fn render(body: &str) -> (String, Option<String>) {
    let opts = Options::ENABLE_TABLES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_FOOTNOTES;
    let mut events: Vec<Event> = Vec::new();
    let mut h1: Option<String> = None;
    let mut in_h1 = false;
    // The open heading (its start event's index, its text) and the ids so far.
    let mut head: Option<(usize, String)> = None;
    let mut ids: Vec<String> = Vec::new();
    // Text gathered for what is written whole: a code block, an image's alt.
    let mut code: Option<(String, String)> = None;
    let mut image: Option<(String, String, String)> = None;
    for e in Parser::new_ext(body, opts) {
        if let Some((_, buf)) = &mut code {
            match e {
                Event::Text(t) => buf.push_str(&t),
                Event::End(TagEnd::CodeBlock) => {
                    let (lang, buf) = code.take().unwrap_or_default();
                    events.push(Event::Html(CowStr::from(block(&lang, &buf))));
                }
                _ => {}
            }
            continue;
        }
        if let Some((_, _, alt)) = &mut image {
            match e {
                Event::Text(t) | Event::Code(t) => alt.push_str(&t),
                Event::End(TagEnd::Image) => {
                    let (src, title, alt) = image.take().unwrap_or_default();
                    let title = match title.is_empty() {
                        true => String::new(),
                        false => format!(" title=\"{}\"", text(&title)),
                    };
                    events.push(Event::InlineHtml(CowStr::from(format!(
                        "<img src=\"{}\" alt=\"{}\"{title}>",
                        url(&src),
                        text(&alt)
                    ))));
                }
                _ => {}
            }
            continue;
        }
        match e {
            Event::Start(Tag::Heading { level, .. }) => {
                in_h1 = level == pulldown_cmark::HeadingLevel::H1 && h1.is_none();
                head = Some((events.len(), String::new()));
                events.push(Event::Start(Tag::Heading {
                    level,
                    id: None,
                    classes: Vec::new(),
                    attrs: Vec::new(),
                }));
            }
            Event::End(TagEnd::Heading(l)) => {
                in_h1 = false;
                if let Some((at, words)) = head.take() {
                    let id = slug(&words, &ids);
                    if !id.is_empty() {
                        events[at] = Event::Start(Tag::Heading {
                            level: l,
                            id: Some(CowStr::from(id.clone())),
                            classes: Vec::new(),
                            attrs: Vec::new(),
                        });
                        ids.push(id);
                    }
                }
                events.push(Event::End(TagEnd::Heading(l)));
            }
            Event::Text(t) => {
                if in_h1 {
                    h1.get_or_insert_default().push_str(&t);
                }
                if let Some((_, words)) = &mut head {
                    words.push_str(&t);
                }
                events.push(Event::InlineHtml(CowStr::from(text(&t))));
            }
            Event::Code(t) => {
                if in_h1 {
                    h1.get_or_insert_default().push_str(&t);
                }
                if let Some((_, words)) = &mut head {
                    words.push_str(&t);
                }
                events.push(Event::InlineHtml(CowStr::from(format!(
                    "<code>{}</code>",
                    text(&t)
                ))));
            }
            Event::Start(Tag::CodeBlock(kind)) => {
                let lang = match kind {
                    CodeBlockKind::Fenced(l) => {
                        l.split([' ', ',', '{']).next().unwrap_or("").to_string()
                    }
                    CodeBlockKind::Indented => String::new(),
                };
                code = Some((lang, String::new()));
            }
            Event::Start(Tag::Image {
                dest_url, title, ..
            }) => {
                image = Some((dest_url.to_string(), title.to_string(), String::new()));
            }
            Event::Start(Tag::Link {
                link_type,
                dest_url,
                title,
                ..
            }) => {
                let mailto = match link_type {
                    LinkType::Email if !dest_url.starts_with("mailto:") => "mailto:",
                    _ => "",
                };
                let title = match title.is_empty() {
                    true => String::new(),
                    false => format!(" title=\"{}\"", text(&title)),
                };
                events.push(Event::InlineHtml(CowStr::from(format!(
                    "<a href=\"{mailto}{}\"{title}>",
                    url(&dest_url)
                ))));
            }
            // A footnote's label is written into `href="#…"` and `id="…"`
            // as it is: its braces would be expressions.
            Event::FootnoteReference(l) => {
                events.push(Event::FootnoteReference(CowStr::from(no_braces(&l))));
            }
            Event::Start(Tag::FootnoteDefinition(l)) => {
                let l = CowStr::from(no_braces(&l));
                events.push(Event::Start(Tag::FootnoteDefinition(l)));
            }
            e => events.push(e),
        }
    }
    let mut html = String::with_capacity(body.len() * 3 / 2);
    pulldown_cmark::html::push_html(&mut html, events.into_iter());
    (html, h1.map(|t| t.trim().to_string()))
}

/// A heading's id, GitHub style: lowercase, letters, digits, `_` and `-`
/// kept, spaces to `-`; `-2`, `-3` after an id already in `taken`.
fn slug(words: &str, taken: &[String]) -> String {
    let base: String = words
        .to_lowercase()
        .chars()
        .filter(|c| c.is_alphanumeric() || matches!(c, '_' | '-' | ' '))
        .collect::<String>()
        .trim()
        .replace(' ', "-");
    let mut id = base.clone();
    let mut n = 2;
    while !base.is_empty() && taken.contains(&id) {
        id = format!("{base}-{n}");
        n += 1;
    }
    id
}

/// A fenced block: `<pre><code class="language-x">`, highlighted when
/// the language is one [`highlight`] knows.
fn block(lang: &str, code: &str) -> String {
    let class = match lang.is_empty() {
        true => String::new(),
        false => format!(" class=\"language-{}\"", text(lang)),
    };
    let body = highlight(lang, code).unwrap_or_else(|| text(code));
    format!("<pre><code{class}>{body}</code></pre>\n")
}

/// What a language's code is made of, for [`highlight`].
struct Lang {
    line: &'static [&'static str],
    block: Option<(&'static str, &'static str)>,
    quotes: &'static [u8],
    keywords: &'static [&'static str],
    /// A capitalized word is a type (Rust, TypeScript).
    types: bool,
    /// `#[derive(Debug)]` is an attribute (Rust).
    attrs: bool,
    /// `/.../` after an operator or `(` is a regex literal, a string to the lexer (JS).
    regex: bool,
}

const RUST: Lang = Lang {
    line: &["//"],
    block: Some(("/*", "*/")),
    quotes: b"\"",
    keywords: &[
        "as", "async", "await", "break", "const", "continue", "crate", "dyn", "else", "enum",
        "extern", "false", "fn", "for", "if", "impl", "in", "let", "loop", "match", "mod", "move",
        "mut", "pub", "ref", "return", "self", "Self", "static", "struct", "super", "trait",
        "true", "type", "unsafe", "use", "where", "while",
    ],
    types: true,
    attrs: true,
    regex: false,
};

const JS: Lang = Lang {
    line: &["//"],
    block: Some(("/*", "*/")),
    quotes: b"\"'`",
    keywords: &[
        "as",
        "async",
        "await",
        "break",
        "case",
        "catch",
        "class",
        "const",
        "continue",
        "default",
        "delete",
        "do",
        "else",
        "export",
        "extends",
        "false",
        "finally",
        "for",
        "from",
        "function",
        "if",
        "import",
        "in",
        "instanceof",
        "interface",
        "let",
        "new",
        "null",
        "of",
        "return",
        "static",
        "switch",
        "this",
        "throw",
        "true",
        "try",
        "type",
        "typeof",
        "undefined",
        "var",
        "void",
        "while",
        "yield",
    ],
    types: true,
    attrs: false,
    regex: true,
};

const CSS: Lang = Lang {
    line: &[],
    block: Some(("/*", "*/")),
    quotes: b"\"'",
    keywords: &["important", "inherit", "initial", "none", "auto"],
    types: false,
    attrs: false,
    regex: false,
};

const JSON: Lang = Lang {
    line: &[],
    block: None,
    quotes: b"\"",
    keywords: &["true", "false", "null"],
    types: false,
    attrs: false,
    regex: false,
};

const TOML: Lang = Lang {
    line: &["#"],
    block: None,
    quotes: b"\"'",
    keywords: &["true", "false"],
    types: false,
    attrs: false,
    regex: false,
};

/// Header values: quoted strings only.
const HTTP: Lang = Lang {
    line: &[],
    block: None,
    quotes: b"\"'",
    keywords: &[],
    types: false,
    attrs: false,
    regex: false,
};

/// Shell words after which the next word is a command again.
const PREFIXES: [&str; 8] = ["sudo", "if", "then", "else", "do", "while", "until", "time"];

/// `code` with its keywords, strings, comments, numbers and types (or an
/// HTML's tags and attributes, a shell's commands and flags, a file tree's
/// paths and notes) in `<span class="hl-k|s|c|n|t|a">`; `None` for a
/// language it does not know.
pub fn highlight(lang: &str, code: &str) -> Option<String> {
    let lang = match lang.to_ascii_lowercase().as_str() {
        "rust" | "rs" => &RUST,
        "js" | "javascript" | "ts" | "typescript" | "jsx" | "tsx" | "mjs" => &JS,
        "css" | "scss" => &CSS,
        "json" => &JSON,
        "bash" | "sh" | "shell" | "zsh" | "console" => return Some(shell(code)),
        "toml" => return Some(lines(code, toml_line)),
        "md" | "markdown" => return Some(lines(code, md_line)),
        "http" => return Some(lines(code, header_line)),
        "tree" => return Some(lines(code, tree_line)),
        "wisp" => return Some(wisp(code)),
        "html"
            if code.starts_with(
                "---
",
            ) =>
        {
            return Some(wisp(code));
        }
        "html" | "xml" | "svg" => return Some(markup(code)),
        _ => return None,
    };
    Some(lex(lang, code))
}

/// Whether a `/` after `before` opens a regex: at the start, or after an operator, `(` or
/// `return`; not after `++`/`--` (an operand ends there) nor right after `<` (a JSX `</a>`).
fn regex_may_start(before: &str) -> bool {
    if before.ends_with('<') {
        return false;
    }
    let t = before.trim_end();
    if t.ends_with("++") || t.ends_with("--") {
        return false;
    }
    t.chars()
        .last()
        .is_none_or(|c| "(,=:[!&|?{};+-*%<>~^".contains(c))
        || ["return", "typeof"].iter().any(|w| {
            t.strip_suffix(w).is_some_and(|p| {
                !p.ends_with(|c: char| c.is_alphanumeric() || c == '_' || c == '$')
            })
        })
}

/// `code` in `lang`: comments, strings, numbers, keywords and types.
fn lex(lang: &Lang, code: &str) -> String {
    let b = code.as_bytes();
    let mut out = String::with_capacity(code.len() * 2);
    let mut i = 0;
    let span = |out: &mut String, class: &str, s: &str| {
        out.push_str(&format!("<span class=\"hl-{class}\">{}</span>", text(s)));
    };
    while i < b.len() {
        let rest = &code[i..];
        if let Some(c) = lang.line.iter().find(|c| rest.starts_with(**c)) {
            // `#` starts a comment in bash only after a space or a line's start.
            let word = *c == "#" && i > 0 && !b[i - 1].is_ascii_whitespace();
            if !word {
                let end = rest.find('\n').unwrap_or(rest.len());
                span(&mut out, "c", &rest[..end]);
                i += end;
                continue;
            }
        }
        if let Some((open, close)) = lang.block
            && rest.starts_with(open)
        {
            let end = rest[open.len()..]
                .find(close)
                .map_or(rest.len(), |e| e + open.len() + close.len());
            span(&mut out, "c", &rest[..end]);
            i += end;
            continue;
        }
        let c = b[i];
        if lang.attrs
            && c == b'#'
            && let Some(open) = rest.find('[')
            && rest[..open].trim_start_matches('#').is_empty()
            && let Some(end) = close_bracket(&rest[open..])
        {
            span(&mut out, "a", &rest[..open + end + 1]);
            i += open + end + 1;
            continue;
        }
        if lang.regex && c == b'/' && regex_may_start(&code[..i]) {
            let mut j = i + 1;
            let mut class = false;
            while j < b.len() && b[j] != b'\n' && (class || b[j] != b'/') {
                match b[j] {
                    b'\\' => j += 1,
                    b'[' => class = true,
                    b']' => class = false,
                    _ => {}
                }
                j += 1;
            }
            if j < b.len() && b[j] == b'/' {
                let flags = code[j + 1..]
                    .find(|ch: char| !ch.is_ascii_alphabetic())
                    .unwrap_or(code.len() - j - 1);
                span(&mut out, "s", &code[i..j + 1 + flags]);
                i = j + 1 + flags;
                continue;
            }
        }
        if lang.quotes.contains(&c) {
            let mut j = i + 1;
            while j < b.len() && b[j] != c {
                j += if b[j] == b'\\' { 2 } else { 1 };
            }
            let end = (j + 1).min(b.len());
            span(&mut out, "s", &code[i..end]);
            i = end;
            continue;
        }
        if c.is_ascii_digit() {
            let end = rest
                .find(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '.' || ch == '_'))
                .unwrap_or(rest.len());
            span(&mut out, "n", &rest[..end]);
            i += end;
            continue;
        }
        if c.is_ascii_alphabetic() || c == b'_' || c == b'$' {
            let end = rest
                .find(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '_' || ch == '$'))
                .unwrap_or(rest.len());
            let word = &rest[..end];
            if lang.keywords.contains(&word) {
                span(&mut out, "k", word);
            } else if lang.types && c.is_ascii_uppercase() {
                span(&mut out, "t", word);
            } else {
                out.push_str(&text(word));
            }
            i += end;
            continue;
        }
        let ch = rest.chars().next().unwrap_or(' ');
        out.push_str(&text(ch.encode_utf8(&mut [0; 4])));
        i += ch.len_utf8();
    }
    out
}

fn paint(out: &mut String, class: &str, s: &str) {
    out.push_str(&format!("<span class=\"hl-{class}\">{}</span>", text(s)));
}

/// Shell: comments, strings, the name of each command (`hl-k`) and its
/// `-flags` (`hl-a`). A lone `$` is a prompt; `A=1` before a command is a setting.
fn shell(code: &str) -> String {
    let b = code.as_bytes();
    let mut out = String::with_capacity(code.len() * 2);
    let (mut i, mut command) = (0, true);
    while i < b.len() {
        let rest = &code[i..];
        let c = b[i];
        if c == b'#' && (i == 0 || b[i - 1].is_ascii_whitespace()) {
            let end = rest.find('\n').unwrap_or(rest.len());
            paint(&mut out, "c", &rest[..end]);
            i += end;
            continue;
        }
        if c == b'"' || c == b'\'' {
            let mut j = i + 1;
            while j < b.len() && b[j] != c {
                j += if b[j] == b'\\' && c == b'"' { 2 } else { 1 };
            }
            let end = (j + 1).min(b.len());
            paint(&mut out, "s", &code[i..end]);
            i = end;
            command = false;
            continue;
        }
        // A line that ends in `\` goes on with the same command.
        if c == b'\\' && rest[1..].starts_with('\n') {
            out.push_str("\\\n");
            i += 2;
            continue;
        }
        if c.is_ascii_whitespace() || b"|;&()".contains(&c) {
            if !matches!(c, b' ' | b'\t' | b'\r') {
                command = true;
            }
            out.push_str(&text(&rest[..1]));
            i += 1;
            continue;
        }
        let end = rest
            .find(|ch: char| ch.is_whitespace() || "|;&()\"'".contains(ch))
            .unwrap_or(rest.len())
            .max(rest.chars().next().map_or(1, char::len_utf8));
        let word = &rest[..end];
        if command && word == "$" {
            out.push('$');
        } else if command && word.contains('=') {
            paint(&mut out, "a", word);
        } else if command {
            paint(&mut out, "k", word);
            command = PREFIXES.contains(&word);
        } else if word.starts_with('-') {
            paint(&mut out, "a", word);
        } else if word.bytes().all(|b| b.is_ascii_digit()) {
            paint(&mut out, "n", word);
        } else {
            out.push_str(&text(word));
        }
        i += end;
    }
    out
}

/// `code` a line at a time, each through `line`.
fn lines(code: &str, line: fn(&mut String, &str)) -> String {
    let mut out = String::with_capacity(code.len() * 2);
    for (i, l) in code.split('\n').enumerate() {
        if i > 0 {
            out.push('\n');
        }
        line(&mut out, l);
    }
    out
}

/// The length of the name `l` opens with when `sep` follows it: a TOML key,
/// a header or a front matter field.
fn key(l: &str, sep: char) -> Option<usize> {
    let k = l
        .find(|c: char| !(c.is_ascii_alphanumeric() || "-_.".contains(c)))
        .unwrap_or(l.len());
    (k > 0 && l[k..].trim_start().starts_with(sep)).then_some(k)
}

/// TOML: `[table]` headers, keys, then values as strings, numbers and booleans.
fn toml_line(out: &mut String, l: &str) {
    let t = l.trim_start();
    out.push_str(&l[..l.len() - t.len()]);
    if t.starts_with('[') {
        let end = t.find(" #").unwrap_or(t.len());
        paint(out, "t", &t[..end]);
        out.push_str(&lex(&TOML, &t[end..]));
    } else if let Some(k) = key(t, '=') {
        paint(out, "a", &t[..k]);
        out.push_str(&lex(&TOML, &t[k..]));
    } else {
        out.push_str(&lex(&TOML, t));
    }
}

/// Markdown: `---` fences, headings and front matter fields.
fn md_line(out: &mut String, l: &str) {
    if l == "---" {
        paint(out, "c", l);
    } else if l.starts_with('#') {
        paint(out, "k", l);
    } else if let Some(k) = key(l, ':') {
        paint(out, "a", &l[..k]);
        out.push_str(&text(&l[k..]));
    } else {
        out.push_str(&text(l));
    }
}

/// HTTP headers: each name, then its value's quoted strings.
fn header_line(out: &mut String, l: &str) {
    let k = key(l, ':').unwrap_or(0);
    if k > 0 {
        paint(out, "a", &l[..k]);
    }
    out.push_str(&lex(&HTTP, &l[k..]));
}

/// A file tree: each path, then (after two spaces or more) what it is.
fn tree_line(out: &mut String, l: &str) {
    let t = l.trim_start();
    out.push_str(&l[..l.len() - t.len()]);
    let end = t.find("  ").unwrap_or(t.len());
    if end > 0 {
        paint(out, "a", &t[..end]);
    }
    let note = t[end..].trim_start();
    out.push_str(&t[end..t.len() - note.len()]);
    if !note.is_empty() {
        paint(out, "c", note);
    }
}

/// A `.wisp` file: the Rust between the `---` lines as Rust, the rest as
/// markup with its `{...}` expressions as Rust.
fn wisp(code: &str) -> String {
    let fence = "<span class=\"hl-c\">---</span>";
    let Some(body) = code.strip_prefix(
        "---
",
    ) else {
        return markup(code);
    };
    let at = if body.starts_with("---") {
        Some(0)
    } else {
        body.find(
            "
---",
        )
        .map(|e| e + 1)
    };
    let Some(at) = at else {
        return format!(
            "{fence}
{}",
            lex(&RUST, body)
        );
    };
    format!(
        "{fence}
{}{fence}{}",
        lex(&RUST, &body[..at]),
        markup(&body[at + 3..])
    )
}

/// The end of the `[...]` that `s` starts with, strings and nesting aside.
fn close_bracket(s: &str) -> Option<usize> {
    let (mut depth, mut quote, mut skip) = (0, false, false);
    for (i, c) in s.char_indices() {
        match c {
            _ if skip => skip = false,
            '\\' if quote => skip = true,
            '"' => quote = !quote,
            '[' if !quote => depth += 1,
            ']' if !quote => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
    }
    None
}

/// The end of the `{...}` that `s` starts with, strings and nesting aside.
fn close(s: &str) -> Option<usize> {
    let (mut depth, mut quote, mut skip) = (0, false, false);
    for (i, c) in s.char_indices() {
        match c {
            _ if skip => skip = false,
            '\\' if quote => skip = true,
            '"' => quote = !quote,
            '{' if !quote => depth += 1,
            '}' if !quote => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
    }
    None
}

/// Markup text or an attribute value with `{expr}`, `{#each x as y}`,
/// `{:else}` and `{/if}` in it: the braces' insides as Rust.
fn braces(out: &mut String, mut s: &str) {
    while let Some(open) = s.find('{') {
        let Some(end) = close(&s[open..]) else { break };
        out.push_str(&text(&s[..open]));
        let inner = &s[open + 1..open + end];
        out.push_str("&#123;");
        let sigil = inner.starts_with(['#', ':', '/', '@']);
        if sigil {
            out.push_str(&text(&inner[..1]));
        }
        let inner = &inner[sigil as usize..];
        let word = if sigil {
            inner
                .find(|c: char| !c.is_ascii_alphanumeric())
                .unwrap_or(inner.len())
        } else {
            0
        };
        if word > 0 {
            out.push_str(&format!("<span class=\"hl-k\">{}</span>", &inner[..word]));
        }
        out.push_str(&lex(&RUST, &inner[word..]));
        out.push_str("&#125;");
        s = &s[open + end + 1..];
    }
    out.push_str(&text(s));
}

/// HTML: comments, tag names, attribute names and quoted values.
fn markup(code: &str) -> String {
    let mut out = String::with_capacity(code.len() * 2);
    let mut rest = code;
    let span = |out: &mut String, class: &str, s: &str| {
        out.push_str(&format!("<span class=\"hl-{class}\">{}</span>", text(s)));
    };
    while let Some(lt) = rest.find('<') {
        braces(&mut out, &rest[..lt]);
        rest = &rest[lt..];
        if rest.starts_with("<!--") {
            let end = rest.find("-->").map_or(rest.len(), |e| e + 3);
            span(&mut out, "c", &rest[..end]);
            rest = &rest[end..];
            continue;
        }
        // `</a>`: the slash before the name, not in it.
        let start = if rest[1..].starts_with('/') { 2 } else { 1 };
        let name_end = rest[start..]
            .find(|c: char| c.is_whitespace() || c == '>' || c == '/')
            .map_or(rest.len(), |e| e + start);
        out.push_str(&rest[..start].replace('<', "&lt;"));
        span(&mut out, "t", &rest[start..name_end]);
        rest = &rest[name_end..];
        // Attributes, to the tag's end.
        while let Some(c) = rest.chars().next() {
            match c {
                '>' => {
                    out.push_str("&gt;");
                    rest = &rest[1..];
                    break;
                }
                '"' | '\'' => {
                    let end = rest[1..].find(c).map_or(rest.len(), |e| e + 2);
                    span(&mut out, "s", &rest[..end]);
                    rest = &rest[end..];
                }
                '{' if close(rest).is_some() => {
                    let end = close(rest).map_or(1, |e| e + 1);
                    braces(&mut out, &rest[..end]);
                    rest = &rest[end..];
                }
                _ if c.is_alphabetic() || c == ':' || c == '@' => {
                    let end = rest
                        .find(|c: char| c.is_whitespace() || matches!(c, '=' | '>' | '/'))
                        .unwrap_or(rest.len());
                    span(&mut out, "a", &rest[..end]);
                    rest = &rest[end..];
                }
                _ => {
                    out.push_str(&text(c.encode_utf8(&mut [0; 4])));
                    rest = &rest[c.len_utf8()..];
                }
            }
        }
    }
    braces(&mut out, rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn front_matter() {
        let (f, start) =
            front("---\ntitle: \"Hi: there\"\n# note\ndate: 2026-10-01\n---\n# Body").unwrap();
        assert_eq!(start, 5);
        assert_eq!(
            f,
            [
                ("title".into(), "Hi: there".into()),
                ("date".into(), "2026-10-01".into())
            ]
        );
        assert_eq!(front("# no front").unwrap(), (Vec::new(), 0));
        assert_eq!(
            front("---\ntitle x\n---").unwrap_err(),
            "2: front matter lines are `name: value`"
        );
        assert!(
            front("---\na: 1\na: 2\n---")
                .unwrap_err()
                .contains("set twice")
        );
        assert!(front("---\na: 1\n").unwrap_err().contains("needs a `---`"));
        assert!(
            front("---\nmy-key: 1\n---")
                .unwrap_err()
                .contains("not a field name")
        );
    }

    #[test]
    fn renders_with_braces_as_references() {
        let md = page(
            "Some {braces} and `code {x}`\n\n```\nfn a() {}\n```\n",
            &[],
            false,
        )
        .unwrap();
        assert!(!md.wisp.contains(['{', '}']), "{}", md.wisp);
        assert!(md.wisp.contains("Some &#123;braces&#125;"), "{}", md.wisp);
        // A footnote's label goes in `href` and `id` as it is.
        let f = page("a[^{x}]\n\n[^{x}]: note\n", &[], false).unwrap().wisp;
        assert!(
            !f.contains(['{', '}']) && f.contains("href=\"#%7Bx%7D\""),
            "{f}"
        );
        assert!(
            md.wisp.contains("<code>code &#123;x&#125;</code>"),
            "{}",
            md.wisp
        );
        assert!(
            md.wisp
                .contains("<pre><code>fn a() &#123;&#125;\n</code></pre>"),
            "{}",
            md.wisp
        );
    }

    #[test]
    fn title_from_front_matter_or_first_heading() {
        let md = page("# Hello `there`\n\ntext\n\n# Second", &[], false).unwrap();
        assert_eq!(md.fields, [("title".into(), "Hello there".into())]);
        assert!(
            md.wisp.starts_with(
                "<head><title>Hello there</title></head>\n<h1 id=\"hello-there\">Hello <code>there</code></h1>"
            ),
            "{}",
            md.wisp
        );
        let md = page("---\ntitle: A <b>\nnoindex: true\n---\n# Other", &[], false).unwrap();
        assert!(md.wisp.starts_with(
            "<head><title>A &lt;b&gt;</title><meta name=\"robots\" content=\"noindex\"></head>\n"
        ), "{}", md.wisp);
        assert!(page("---\nnoindex: yes\n---\n", &[], false).is_err());
        // A layout above writes the one title (it reads this one).
        let md = page("---\ntitle: A\nnoindex: true\n---\n", &[], true).unwrap();
        assert!(
            md.wisp
                .starts_with("<head><meta name=\"robots\" content=\"noindex\"></head>\n"),
            "{}",
            md.wisp
        );
    }

    #[test]
    fn wisp_blocks_highlight_rust_and_expressions() {
        let h = highlight(
            "wisp",
            "---
let n = 1;
---
<p class={c}>{#each xs as x}{x}{/each}</p>",
        )
        .unwrap();
        assert!(
            h.starts_with(
                "<span class=\"hl-c\">---</span>
<span class=\"hl-k\">let</span> n = <span class=\"hl-n\">1</span>;
<span class=\"hl-c\">---</span>"
            ),
            "{h}"
        );
        assert!(
            h.contains(
                "&#123;#<span class=\"hl-k\">each</span> xs <span class=\"hl-k\">as</span> x&#125;"
            ),
            "{h}"
        );
        assert!(
            h.contains("<span class=\"hl-a\">class</span>=&#123;c&#125;"),
            "{h}"
        );
        assert_eq!(
            highlight("html", "<b>{x}</b>").unwrap(),
            "&lt;<span class=\"hl-t\">b</span>&gt;&#123;x&#125;&lt;/<span class=\"hl-t\">b</span>&gt;"
        );
        assert!(
            highlight(
                "html",
                "---
fn a() {}
---
"
            )
            .unwrap()
            .contains("hl-k\">fn")
        );
    }

    #[test]
    fn headings_get_ids() {
        let md = page(
            "## Install and Run\n\n## `wisp::pages()`, again!\n\n## Install and Run\n\n## ?",
            &[],
            false,
        )
        .unwrap();
        for h in [
            "<h2 id=\"install-and-run\">",
            "<h2 id=\"wisppages-again\">",
            "<h2 id=\"install-and-run-2\">",
            "<h2>?</h2>",
        ] {
            assert!(md.wisp.contains(h), "{h} in {}", md.wisp);
        }
    }

    #[test]
    fn components_and_links_pass_through() {
        let src = "<Card title=\"x\">\n\nSome *markdown*.\n\n</Card>\n\n[a {b}](/x \"t\") ![alt {c}](/i.png) <me@x.org>";
        let md = page(src, &[], false).unwrap();
        assert!(
            md.wisp
                .contains("<Card title=\"x\">\n<p>Some <em>markdown</em>.</p>\n</Card>"),
            "{}",
            md.wisp
        );
        assert!(
            md.wisp
                .contains("<a href=\"/x\" title=\"t\">a &#123;b&#125;</a>"),
            "{}",
            md.wisp
        );
        assert!(
            md.wisp
                .contains("<img src=\"/i.png\" alt=\"alt &#123;c&#125;\">"),
            "{}",
            md.wisp
        );
        assert!(
            md.wisp.contains("<a href=\"mailto:me@x.org\">"),
            "{}",
            md.wisp
        );
    }

    #[test]
    fn layout_props_by_type() {
        use crate::template::PropDecl;
        let decl = |name: &str, ty: &str, default: Option<&str>| PropDecl {
            name: name.into(),
            ty: ty.into(),
            default: default.map(Into::into),
        };
        let comp = Comp {
            rel: "src/components/Post.wisp".into(),
            name: "Post".into(),
            module: "c_post".into(),
            props: vec![
                decl("title", "&str", None),
                decl("n", "u32", Some("0")),
                decl("draft", "bool", Some("false")),
                decl("tag", "Option<&str>", None),
            ],
            children: true,
            bindable: None,
            rest: false,
            live: false,
        };
        let comps = [comp];
        let md = page(
            "---\nlayout: Post\ntitle: Hi {x}\nn: 3\ndraft: true\nextra: y\n---\nBody",
            &comps,
            false,
        )
        .unwrap();
        assert!(
            md.wisp
                .contains("<Post title={\"Hi {x}\"} n={3_u32} draft={true}>\n<p>Body</p>\n</Post>"),
            "{}",
            md.wisp
        );
        assert_eq!(md.fields.len(), 5);
        let e = |src: &str| page(src, &comps, false).unwrap_err();
        assert!(e("---\nlayout: Post\n---\n").contains("needs `title`"));
        assert!(
            e("---\nlayout: Post\ntitle: a\nn: x\n---\n").contains("cannot be Post's `n: u32`")
        );
        assert!(e("---\nlayout: Nope\n---\n").contains("src/components/Nope.wisp"));
    }

    #[test]
    fn highlights_common_languages() {
        let h = highlight(
            "rust",
            "fn main() { let s = \"a\"; } // hi\nVec::<u8>::new(1)",
        )
        .unwrap();
        assert_eq!(
            h,
            "<span class=\"hl-k\">fn</span> main() &#123; <span class=\"hl-k\">let</span> s = \
             <span class=\"hl-s\">&quot;a&quot;</span>; &#125; <span class=\"hl-c\">// hi</span>\n\
             <span class=\"hl-t\">Vec</span>::&lt;u8&gt;::new(<span class=\"hl-n\">1</span>)"
        );
        let h = highlight("html", "<a href=\"/x\">hi</a><!-- c -->").unwrap();
        assert_eq!(
            h,
            "&lt;<span class=\"hl-t\">a</span> <span class=\"hl-a\">href</span>=<span class=\"hl-s\">&quot;/x&quot;</span>&gt;hi\
             &lt;/<span class=\"hl-t\">a</span>&gt;<span class=\"hl-c\">&lt;!-- c --&gt;</span>"
        );
        let h = highlight("bash", "echo a#b # c").unwrap();
        assert_eq!(
            h,
            "<span class=\"hl-k\">echo</span> a#b <span class=\"hl-c\"># c</span>"
        );
        let h = highlight(
            "sh",
            "$ A=1 cargo add x --features h2 # c\nwisp dev | grep 'ok'",
        )
        .unwrap();
        assert_eq!(
            h,
            "$ <span class=\"hl-a\">A=1</span> <span class=\"hl-k\">cargo</span> add x \
             <span class=\"hl-a\">--features</span> h2 <span class=\"hl-c\"># c</span>\n\
             <span class=\"hl-k\">wisp</span> dev | <span class=\"hl-k\">grep</span> <span class=\"hl-s\">'ok'</span>"
        );
        let h = highlight("toml", "[deps]\nwisp = { version = \"1\" } # c").unwrap();
        assert_eq!(
            h,
            "<span class=\"hl-t\">[deps]</span>\n<span class=\"hl-a\">wisp</span> = &#123; version = \
             <span class=\"hl-s\">&quot;1&quot;</span> &#125; <span class=\"hl-c\"># c</span>"
        );
        let h = highlight("tree", "src/main.rs   the app\n  x/").unwrap();
        assert_eq!(
            h,
            "<span class=\"hl-a\">src/main.rs</span>   <span class=\"hl-c\">the app</span>\n  <span class=\"hl-a\">x/</span>"
        );
        let h = highlight("markdown", "---\ntitle: Hi\n---\n# Hi").unwrap();
        assert!(
            h.starts_with("<span class=\"hl-c\">---</span>\n<span class=\"hl-a\">title</span>: Hi")
        );
        assert!(h.ends_with("<span class=\"hl-k\"># Hi</span>"));
        let h = highlight("http", "a-b: x 'y'\n  z").unwrap();
        assert!(h.starts_with("<span class=\"hl-a\">a-b</span>: x <span class=\"hl-s\">"));
        // A character no rule takes still moves the lexer on.
        assert!(highlight("sh", "a \u{a0}b").is_some());
        assert!(highlight("cobol", "x").is_none());
        for lang in ["js", "ts", "css", "json"] {
            assert!(highlight(lang, "/* x */ \"y\" 1 true").is_some());
        }
        // A quote inside a regex literal does not open a string; a division is not a regex.
        let h = highlight("js", r"if (!/^[a-z'\/]+$/i.test(x)) y = 'z';").unwrap();
        assert!(h.contains(r#"<span class="hl-s">/^[a-z'\/]+$/i</span>"#));
        assert!(h.contains("<span class=\"hl-s\">'z'</span>"));
        assert!(!highlight("js", "a = b / c / d;").unwrap().contains("hl-s"));
        // `++`/`--` end an operand, `return` must be a whole word, `</` closes a JSX tag.
        for code in [
            "i++ / 2 / 3;",
            "x-- / 2 / 3;",
            "noreturn / 2 / 3;",
            "<a>x</a> <b>y</b>;",
        ] {
            assert!(!highlight("js", code).unwrap().contains("hl-s"), "{code}");
        }
        assert!(
            highlight("js", "return /a/.test(x);")
                .unwrap()
                .contains("hl-s")
        );
        assert!(
            page("```rust\nlet x = 1;\n```", &[], false)
                .unwrap()
                .wisp
                .contains("<pre><code class=\"language-rust\"><span class=\"hl-k\">let</span>")
        );
    }
}
