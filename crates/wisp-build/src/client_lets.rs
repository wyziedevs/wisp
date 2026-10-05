//! A `---` block's `let name = literal;` that only the browser reads
//! (`{:name}`, `on:`, `bind:`, the client script) is browser state: the
//! build moves it into the file's client script, so the page compiles as if
//! it were written `<script>let name = literal</script>`. The server then
//! computes, serializes and sends nothing for it.
//!
//! Only what means the same in Rust and JavaScript moves: numbers without a
//! suffix, `true`, `false`, plain strings, `None` (`null`) and arrays of
//! those (`[..]` or `vec![..]`). Anything else, and any name the server
//! reads too, stays a server value.

use wisp_shared::rust::skip_literal;

/// Folds `rust`'s browser-only literal lets into `markup` (both as long as
/// the file, line for line; `open` and `close` are the `---` lines), and
/// says whether any moved.
pub(crate) fn fold(rust: &mut String, markup: &mut String, open: usize, close: usize) -> bool {
    let lets = candidates(rust);
    if lets.is_empty() {
        return false;
    }
    // What the browser runs: directive values, `{:…}` holes and blocks, and
    // the script. Markup that does not parse is left for the build to say.
    let Ok(t) = crate::template::parse(markup) else {
        return false;
    };
    let mut client = String::new();
    for d in t.groups.iter().flat_map(|g| &g.directives) {
        if let Some(v) = &d.value {
            client.push_str(&v.src);
            client.push('\n');
        }
    }
    let declared = t.script.as_ref().map_or_else(Vec::new, |s| {
        client.push_str(&s.src);
        crate::js::declarations(&s.src)
    });
    let server = server_code(markup);
    let script = client_script(markup);
    let mut moved: Vec<(usize, String)> = Vec::new();
    for l in &lets {
        let others = word_count(rust, &l.name) - 1 + word_count(&server, &l.name);
        let read = word_count(&client, &l.name) > 0;
        if read && others == 0 && !declared.iter().any(|(d, _)| *d == l.name) {
            moved.push((l.line, format!("let {} = {};", l.name, l.js)));
        }
    }
    if moved.is_empty() {
        return false;
    }
    // The Rust loses those lines, kept empty.
    let mut lines: Vec<String> = rust.split('\n').map(str::to_string).collect();
    for (line, _) in &moved {
        lines[*line].clear();
    }
    *rust = lines.join("\n");
    match script {
        // Into the file's own script, first, on its first line: no line
        // moves, and the script's code comes after what it may read.
        Some((body, _)) => {
            let decls: String = moved.iter().map(|(_, d)| d.as_str()).collect();
            markup.insert_str(body, &decls);
        }
        // A script where the block was, each `let` on its line.
        None => {
            let mut lines: Vec<String> = markup.split('\n').map(str::to_string).collect();
            lines[open] = "<script>".into();
            for (line, decl) in moved {
                lines[line] = decl;
            }
            lines[close] = "</script>".into();
            *markup = lines.join("\n");
        }
    }
    true
}

/// A top-level `let name = literal;` on one line.
struct Let {
    line: usize,
    name: String,
    js: String,
}

/// The block's statements at depth 0 that are a literal `let` on one line.
fn candidates(rust: &str) -> Vec<Let> {
    let b = rust.as_bytes();
    let mut out = Vec::new();
    let (mut i, mut depth, mut start) = (0, 0i32, 0);
    while i < b.len() {
        match b[i] {
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' => depth -= 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    start = i + 1;
                }
            }
            b';' if depth == 0 => {
                let stmt = &rust[start..=i];
                let text = stmt.trim_start();
                // On a line of its own: nothing else before it on its line.
                let at = start + stmt.len() - text.len();
                let alone = rust[..at]
                    .rsplit('\n')
                    .next()
                    .is_some_and(|l| l.trim().is_empty());
                // And nothing after it but a comment: its line is cleared.
                let after = rust[i + 1..].split('\n').next().unwrap_or("").trim();
                if alone
                    && (after.is_empty() || after.starts_with("//"))
                    && !text.contains('\n')
                    && let Some(l) = literal_let(text, rust[..at].matches('\n').count())
                {
                    out.push(l);
                }
                start = i + 1;
            }
            _ => i = skip_literal(b, i),
        }
        i += 1;
    }
    out
}

/// `let [mut] name[: Type] = literal;` as a [`Let`] on `line`.
fn literal_let(s: &str, line: usize) -> Option<Let> {
    let s = s.strip_prefix("let ")?.strip_suffix(';')?.trim_start();
    let s = s.strip_prefix("mut ").unwrap_or(s).trim_start();
    let end = s
        .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .unwrap_or(s.len());
    let name = &s[..end];
    let first = name.chars().next()?;
    if !(first.is_ascii_alphabetic() || first == '_') || name == "_" || is_keyword(name) {
        return None;
    }
    let rest = s[end..].trim_start();
    let rest = match rest.strip_prefix(':') {
        Some(ty) => &ty[ty.find('=')?..],
        None => rest,
    };
    let init = rest.strip_prefix('=')?.trim();
    let mut p = Lit {
        s: init.as_bytes(),
        i: 0,
        out: String::new(),
    };
    p.value()?;
    p.space();
    (p.i == init.len()).then(|| Let {
        line,
        name: name.into(),
        js: p.out,
    })
}

fn is_keyword(w: &str) -> bool {
    crate::js::is_reserved(w) || matches!(w, "self" | "Self" | "crate" | "super" | "cx" | "data")
}

/// A Rust literal read as the JavaScript that means the same.
struct Lit<'a> {
    s: &'a [u8],
    i: usize,
    out: String,
}

impl Lit<'_> {
    fn space(&mut self) {
        while self.s.get(self.i).is_some_and(u8::is_ascii_whitespace) {
            self.i += 1;
        }
    }

    fn word(&mut self, w: &str) -> bool {
        let rest = &self.s[self.i..];
        let ends = rest
            .get(w.len())
            .is_none_or(|c| !(c.is_ascii_alphanumeric() || *c == b'_' || *c == b'!'));
        if rest.starts_with(w.as_bytes()) && ends {
            self.i += w.len();
            true
        } else {
            false
        }
    }

    fn value(&mut self) -> Option<()> {
        self.space();
        let c = *self.s.get(self.i)?;
        for (rust, js) in [("true", "true"), ("false", "false"), ("None", "null")] {
            if self.word(rust) {
                self.out.push_str(js);
                return Some(());
            }
        }
        if self.s[self.i..].starts_with(b"vec!") {
            self.i += 4;
            self.space();
            return (self.s.get(self.i) == Some(&b'['))
                .then(|| self.array())
                .flatten();
        }
        match c {
            b'[' => self.array(),
            b'"' => self.string(),
            b'-' | b'0'..=b'9' => self.number(),
            _ => None,
        }
    }

    fn array(&mut self) -> Option<()> {
        self.i += 1;
        self.out.push('[');
        loop {
            self.space();
            if self.s.get(self.i) == Some(&b']') {
                self.i += 1;
                self.out.push(']');
                return Some(());
            }
            self.value()?;
            self.space();
            match self.s.get(self.i)? {
                b',' => {
                    self.i += 1;
                    self.out.push_str(", ");
                }
                b']' => {}
                _ => return None,
            }
        }
    }

    /// A string with nothing to escape and nothing that could end a
    /// `<script>`: no `\`, `"` inside, `<` or line break.
    fn string(&mut self) -> Option<()> {
        let rest = &self.s[self.i + 1..];
        let len = rest.iter().position(|&c| c == b'"')?;
        let body = &rest[..len];
        if body
            .iter()
            .any(|&c| matches!(c, b'\\' | b'<' | b'\n' | b'\r'))
        {
            return None;
        }
        let text = std::str::from_utf8(body).ok()?;
        if text.contains(['\u{2028}', '\u{2029}']) {
            return None;
        }
        self.out.push('"');
        self.out.push_str(text);
        self.out.push('"');
        self.i += len + 2;
        Some(())
    }

    /// A decimal number without a suffix (`1`, `-2.5`, `1_000`, `1e3`),
    /// written without `_`. A leading `0` before a digit is octal in
    /// JavaScript, so it stays Rust.
    fn number(&mut self) -> Option<()> {
        let start = self.i;
        if self.s[self.i] == b'-' {
            self.i += 1;
        }
        let digits = self.i;
        while self
            .s
            .get(self.i)
            .is_some_and(|c| c.is_ascii_digit() || matches!(c, b'_' | b'.' | b'e' | b'E'))
            || (matches!(self.s.get(self.i), Some(b'+' | b'-'))
                && matches!(self.s[self.i - 1], b'e' | b'E'))
        {
            self.i += 1;
        }
        let n: String = self.s[start..self.i]
            .iter()
            .filter(|&&c| c != b'_')
            .map(|&c| c as char)
            .collect();
        let body = &n[digits - start..];
        let ok = body.starts_with(|c: char| c.is_ascii_digit())
            && !(body.len() > 1 && body.starts_with('0') && body.as_bytes()[1].is_ascii_digit())
            && body.matches('.').count() <= 1
            && n.parse::<f64>().is_ok()
            && !self
                .s
                .get(self.i)
                .is_some_and(|c| c.is_ascii_alphanumeric() || *c == b'_');
        ok.then(|| self.out.push_str(&n))
    }
}

/// The markup's Rust: its `{…}` holes that are not the browser's (`{:…}`),
/// outside the client script, `<style>` and comments.
fn server_code(markup: &str) -> String {
    let b = markup.as_bytes();
    let mut out = String::new();
    let mut i = 0;
    while i < b.len() {
        let rest = &b[i..];
        let tag = |t: &[u8]| rest.len() > t.len() && rest[..t.len()].eq_ignore_ascii_case(t);
        if tag(b"<script") || tag(b"<style") {
            let end: &[u8] = if tag(b"<script") {
                b"</script"
            } else {
                b"</style"
            };
            i += find(rest, end).map_or(rest.len(), |e| e + end.len());
        } else if rest.starts_with(b"<!--") {
            i += find(rest, b"-->").map_or(rest.len(), |e| e + 3);
        } else if b[i] == b'{' {
            let end = crate::template::hole_end(b, i + 1).unwrap_or(b.len());
            if b.get(i + 1) != Some(&b':') {
                out.push_str(&markup[i + 1..end]);
                out.push('\n');
            }
            i = end + 1;
        } else {
            i += 1;
        }
    }
    out
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len())
        .position(|w| w.eq_ignore_ascii_case(needle))
}

/// The body of the file's client script (a `<script>` with no attribute
/// but `lang`), as its start and end.
fn client_script(markup: &str) -> Option<(usize, usize)> {
    let mut from = 0;
    while let Some(k) = find(&markup.as_bytes()[from..], b"<script") {
        let at = from + k + "<script".len();
        let close = at + markup[at..].find('>')?;
        let attrs = markup[at..close].trim();
        if attrs.is_empty() || attrs.starts_with("lang") {
            let body = close + 1;
            let end = body + find(&markup.as_bytes()[body..], b"</script")?;
            return Some((body, end));
        }
        from = close;
    }
    None
}

/// How many times `name` is in `code` as a whole word.
fn word_count(code: &str, name: &str) -> usize {
    let ident = |c: u8| c.is_ascii_alphanumeric() || c == b'_';
    let b = code.as_bytes();
    code.match_indices(name)
        .filter(|&(k, _)| {
            (k == 0 || !ident(b[k - 1])) && b.get(k + name.len()).is_none_or(|&c| !ident(c))
        })
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn js(init: &str) -> Option<String> {
        literal_let(&format!("let x = {init};"), 0).map(|l| l.js)
    }

    #[test]
    fn literals_read_as_javascript() {
        for (rust, want) in [
            ("0", "0"),
            ("-12", "-12"),
            ("1_000", "1000"),
            ("2.5", "2.5"),
            ("1e3", "1e3"),
            ("true", "true"),
            ("false", "false"),
            ("None", "null"),
            ("\"hi there\"", "\"hi there\""),
            ("[1, 2]", "[1, 2]"),
            ("vec![\"a\", \"b\"]", "[\"a\", \"b\"]"),
            ("vec![]", "[]"),
            ("[[1], [2, 3]]", "[[1], [2, 3]]"),
        ] {
            assert_eq!(js(rust).as_deref(), Some(want), "{rust}");
        }
        for rust in [
            "0u32",
            "07",
            "1.2.3",
            "\"a\\nb\"",
            "\"</script>\"",
            "x",
            "f()",
            "String::new()",
            "1 + 1",
            "'a'",
            "r\"a\"",
            "[1, x]",
            "vec![1; 3]",
            "Some(1)",
            "true_",
            "-",
        ] {
            assert_eq!(js(rust), None, "{rust}");
        }
        assert_eq!(
            literal_let("let mut n: u32 = 5;", 3).map(|l| (l.name, l.js, l.line)),
            Some(("n".into(), "5".into(), 3))
        );
        assert!(literal_let("let (a, b) = (1, 2);", 0).is_none());
    }

    fn folded(src: &str) -> (String, String) {
        let (rust, markup) = crate::split_front(src).unwrap();
        (rust.unwrap_or_default(), markup)
    }

    #[test]
    fn browser_only_literals_move_to_the_script() {
        let (rust, markup) = folded(
            "---\nlet count = 0;\nlet who = cx.query_or(\"who\", \"me\");\nlet shown = 1;\n---\n\
             <p>{shown} {who}</p><button on:click=\"count++\">{:count}</button>",
        );
        assert_eq!(
            rust,
            "\n\nlet who = cx.query_or(\"who\", \"me\");\nlet shown = 1;\n\n"
        );
        assert_eq!(
            markup,
            "<script>\nlet count = 0;\n\n\n</script>\n<p>{shown} {who}</p><button on:click=\"count++\">{:count}</button>"
        );
    }

    #[test]
    fn into_the_file_s_own_script() {
        let (rust, markup) = folded(
            "---\nlet n = 2;\n---\n<b on:click=\"n++\">{:big}</b>\n<script>\nconst big = $derived(n > 5)\n</script>",
        );
        assert!(rust.trim().is_empty(), "{rust}");
        assert!(markup.contains("<script>let n = 2;\nconst big"), "{markup}");
    }

    #[test]
    fn what_the_server_reads_stays() {
        for src in [
            // Read in Rust, in the block or the markup.
            "---\nlet n = 1;\nlet m = n + 1;\n---\n{:m}",
            "---\nlet n = 1;\n---\n<p>{n}</p>{:n}",
            "---\nlet n = 1;\n---\n{#if n > 0}{:n}{/if}",
            // Inside a function, not the block's statement.
            "---\nfn f() {\n    let n = 1;\n}\n---\n{:n}",
            // Nothing in the browser reads it.
            "---
let n = 1;
---
<p>n</p>",
            // The script declares its own.
            "---\nlet n = 1;\n---\n{:n}<script>let n = 2</script>",
        ] {
            let (rust, _) = folded(src);
            assert!(rust.contains("let n = 1;"), "{src}");
        }
    }

    #[test]
    fn what_shares_its_line_is_kept() {
        // Moving `n` must not take the rest of its line with it.
        for src in [
            "---\nlet n = 1; let m = 2;\n---\n{m}{:n}",
            "---\nlet n = 1; cx.status(201);\n---\n{:n}",
        ] {
            let (rust, _) = folded(src);
            assert!(rust.contains("let n = 1;"), "{src}: {rust}");
        }
        // A trailing comment is fine to drop with it.
        let (rust, markup) = folded("---\nlet n = 1; // start\n---\n{:n}");
        assert!(!rust.contains("let n"), "{rust}");
        assert!(markup.contains("let n = 1;"), "{markup}");
    }

    #[test]
    fn edge_lets() {
        for (src, moves) in [
            ("---\nlet n = -2.5e-3;\n---\n{:n}", true),
            ("---\nlet n = 0x10;\n---\n{:n}", false),
            ("---\nlet n = \"a{b}c\";\n---\n{:n}", true),
            ("---\nlet n = \"é ✓\";\n---\n{:n}", true),
            ("---\nlet n = \"a\\\"b\";\n---\n{:n}", false),
            ("---\nlet n = r#\"x\"#;\n---\n{:n}", false),
            ("---\nlet n = Some(1);\n---\n{:n}", false),
            ("---\nlet n = 1;\nlet n = 2;\n---\n{:n}", false),
            ("---\nlet café = 1;\n---\n{:café}", false),
            ("---\nlet n = /* c */ 1;\n---\n{:n}", false),
            ("---\nlet n: Vec<i32> = vec![];\n---\n{:n}", true),
            (
                "---\nlet n = 1;\n#[action]\nfn go(cx: &mut Cx) { let _ = n; }\n---\n{:n}",
                false,
            ),
            ("---\nlet n = 1;\n---\n{:n}<SCRIPT>let m = 2</SCRIPT>", true),
        ] {
            let (rust, markup) = folded(src);
            assert_eq!(
                !rust.trim_start().starts_with("let"),
                moves,
                "{src}: {rust} | {markup}"
            );
            if moves {
                assert!(crate::template::parse(&markup).is_ok(), "{markup}");
            }
        }
    }

    #[test]
    fn fuzz_never_panics() {
        let parts = [
            "let ",
            "mut ",
            "n",
            " = ",
            "-",
            "1",
            "_",
            ".",
            "e",
            "\"",
            "{",
            "}",
            "(",
            ")",
            "[",
            "]",
            ";",
            "\n",
            "vec!",
            ",",
            "None",
            "//",
            "/*",
            "*/",
            "é",
            "'",
            "r#",
            ":",
            "<script>",
            "</script>",
            "{:n}",
            "---\n",
            " ",
        ];
        let mut seed = 0x2545_f491_4f6c_dd1du64;
        for _ in 0..20_000 {
            let mut s = String::from("---\n");
            for _ in 0..(seed % 24) {
                seed ^= seed << 13;
                seed ^= seed >> 7;
                seed ^= seed << 17;
                s.push_str(parts[(seed % parts.len() as u64) as usize]);
            }
            s.push_str("\n---\n{:n}<p on:click=\"n++\"></p>");
            let _ = crate::split_front(&s);
        }
    }
}
