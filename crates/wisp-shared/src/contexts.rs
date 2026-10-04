//! Every rule of where a value lands in a page, in one place: what text
//! and quoted attribute values escape, which attributes hold a URL and
//! which URLs run script.
//!
//! The runtime renders by it and `wisp-build` folds literal holes and
//! checks templates by it, byte for byte the same. The browser runtime's
//! half (`client/live.js`, `client/wisp.js`, `client/extra.js`) is held to
//! the same case table by the tests below, under Node. The hot ones are
//! `#[inline]`: other crates call them, with or without LTO.

/// Appends `s` with `& < > " '` escaped: safe in text and in quoted attributes.
#[inline]
pub fn escape(out: &mut String, s: &str) {
    let bytes = s.as_bytes();
    let mut done = 0; // `s[..done]` is in `out` already
    // 16 bytes at a time. The loop has no branch or early exit, so it
    // compiles to a few vector compares, and a chunk with nothing to escape
    // (the usual case) is skipped without looking at its bytes one by one:
    // 3.5x the speed of a byte loop on plain text.
    let (chunks, rest) = bytes.as_chunks::<16>();
    for (n, chunk) in chunks.iter().enumerate() {
        if hit(chunk) {
            escape_bytes(out, s, &mut done, n * 16, n * 16 + 16);
        }
    }
    // The last few bytes, tested as the last 16 (which overlap the chunk
    // before) when there are that many: a clean tail is not looked at one
    // byte at a time either.
    if !rest.is_empty() && bytes.last_chunk().is_none_or(hit) {
        escape_bytes(out, s, &mut done, bytes.len() - rest.len(), bytes.len());
    }
    out.push_str(&s[done..]);
}

#[inline(always)]
fn hit(chunk: &[u8; 16]) -> bool {
    let mut hit = 0u8;
    for &b in chunk {
        hit |= special(b) as u8;
    }
    hit != 0
}

/// `& ' < > "` in three compares: `&` and `'` differ only in bit 0, `<` and
/// `>` only in bit 1.
#[inline(always)]
fn special(b: u8) -> bool {
    ((b | 1) == b'\'') | ((b | 2) == b'>') | (b == b'"')
}

/// What `escape` writes for each byte it escapes.
const ESCAPED: [&str; 5] = ["&amp;", "&lt;", "&gt;", "&quot;", "&#39;"];

/// Escapes `s[from..to]`, appending what precedes each escaped byte first.
#[inline(always)]
fn escape_bytes(out: &mut String, s: &str, done: &mut usize, from: usize, to: usize) {
    for (i, &b) in s.as_bytes()[from..to].iter().enumerate() {
        let replacement = match b {
            b'&' => ESCAPED[0],
            b'<' => ESCAPED[1],
            b'>' => ESCAPED[2],
            b'"' => ESCAPED[3],
            b'\'' => ESCAPED[4],
            _ => continue,
        };
        // Escaped bytes are ASCII, so `from + i` is a char boundary.
        out.push_str(&s[*done..from + i]);
        out.push_str(replacement);
        *done = from + i + 1;
    }
}

/// Attributes whose value is a URL, where `javascript:` would run script.
/// live.js's `URLS` is the same list (the tests hold it to this one).
pub const URL_ATTRS: [&str; 9] = [
    "action",
    "background",
    "cite",
    "data",
    "formaction",
    "href",
    "poster",
    "src",
    "xlink:href",
];

/// Whether attribute `name` (any case) holds a URL. Each compare looks at
/// the lengths first, so a name of no URL attribute's length costs nine
/// integer compares: faster than switching on its length and first byte,
/// measured.
#[inline]
pub fn is_url_attr(name: &str) -> bool {
    URL_ATTRS.iter().any(|a| a.eq_ignore_ascii_case(name))
}

/// What a URL that would run script is replaced with, as React and Angular
/// do.
pub const BLOCKED: &str = "about:invalid#blocked";

/// What the start of a URL says about its scheme.
#[derive(Debug, PartialEq, Eq)]
pub enum Scheme {
    /// Relative (`/x`, `?q`, `#top`), or a scheme that runs no script.
    Fixed,
    /// `javascript:` or `vbscript:`.
    Script,
    /// A character reference before the scheme ends, which a browser
    /// decodes to who knows what.
    Encoded,
    /// Undecided: what comes next decides it.
    Open,
}

/// Reads `url` as it stands in the page (escaped, or a template's own
/// text) the way a browser does: leading spaces and controls dropped, tabs
/// and newlines ignored, the scheme being what comes before a `:` if every
/// byte before it can be in one. A character reference `escape` writes
/// (`&amp;`...) decodes to a byte no scheme has: the URL is relative.
#[inline]
pub fn scheme(url: &str) -> Scheme {
    let mut name = [0u8; 10];
    let mut n = 0;
    let b = url.trim_start_matches(|c: char| c <= ' ').as_bytes();
    for (i, &c) in b.iter().enumerate() {
        match c {
            b'\t' | b'\n' | b'\r' => {}
            b':' if matches!(&name[..n], b"javascript" | b"vbscript") => return Scheme::Script,
            b'&' if !ESCAPED.iter().any(|e| b[i..].starts_with(e.as_bytes())) => {
                return Scheme::Encoded;
            }
            _ if c.is_ascii_alphabetic()
                || (n > 0 && (c.is_ascii_digit() || matches!(c, b'+' | b'-' | b'.'))) =>
            {
                if n == name.len() {
                    return Scheme::Fixed; // longer than any scheme that runs script
                }
                name[n] = c.to_ascii_lowercase();
                n += 1;
            }
            // Another scheme's `:`, or not a scheme's byte: a relative URL.
            _ => return Scheme::Fixed,
        }
    }
    Scheme::Open
}

/// Whether `url` (escaped) would run script when followed.
#[inline]
pub fn runs_script(url: &str) -> bool {
    matches!(scheme(url), Scheme::Script | Scheme::Encoded)
}

/// Ends the value of a URL attribute (`href`, `src`, `action`...) that
/// began at `out[start..]` and whose scheme an expression chose. A value
/// that would run script when followed is replaced with `BLOCKED`, so
/// `href={link}` is safe whatever `link` holds.
#[inline]
pub fn guard_url(out: &mut String, start: usize) {
    if runs_script(&out[start..]) {
        out.truncate(start);
        out.push_str(BLOCKED);
    }
}

/// Ends the value of attribute `name` that began at `out[start..]`, any
/// value at all: guarded as `guard_url` does when `name` holds a URL.
#[inline]
pub fn guard_attr(out: &mut String, name: &str, start: usize) {
    if is_url_attr(name) {
        guard_url(out, start);
    }
}

/// What an attribute holds that escaping does not make safe, so that no
/// value an expression chose may go in it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Held {
    /// `on…`: script.
    Event,
    /// `srcdoc`: a whole HTML document.
    Document,
    /// A URL that may run script without being a URL attribute: an SVG
    /// `<animate>`/`<set>`'s `to from values by` (which can set an `href`),
    /// a `<meta>`'s `http-equiv` (which can make its `content` a refresh).
    Url,
    /// A `<meta>`'s `content`: a URL when its `http-equiv` is refresh.
    Refresh,
}

/// What attribute `name` of a `tag` holds that escaping does not make safe,
/// both in any case. A `tag` of "" is one the browser's code chooses: any
/// tag at all. Templates refuse expressions in these; spreads leave them
/// out (extra.js's `held` is the same rule, held to it by the tests).
pub fn holds_script(tag: &str, name: &str) -> Option<Held> {
    let n = name.as_bytes();
    let is = |t: &str| tag.is_empty() || tag.eq_ignore_ascii_case(t);
    let any = |list: &[&str]| list.iter().any(|a| a.eq_ignore_ascii_case(name));
    if n.len() >= 2 && n[..2].eq_ignore_ascii_case(b"on") {
        Some(Held::Event)
    } else if name.eq_ignore_ascii_case("srcdoc") {
        Some(Held::Document)
    } else if ((is("animate") || is("set")) && any(&["to", "from", "values", "by"]))
        || (is("meta") && name.eq_ignore_ascii_case("http-equiv"))
    {
        Some(Held::Url)
    } else if is("meta") && name.eq_ignore_ascii_case("content") {
        Some(Held::Refresh)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Attributes and whether a spread may set them, on a tag.
    const HELD: &[(&str, &str, bool)] = &[
        ("a", "onclick", true),
        ("a", "ONCLICK", true),
        ("a", "OnMouseOver", true),
        ("iframe", "srcdoc", true),
        ("iframe", "SRCDOC", true),
        ("set", "to", true),
        ("SET", "TO", true),
        ("animate", "values", true),
        ("animate", "by", true),
        ("animate", "from", true),
        ("meta", "content", true),
        ("meta", "HTTP-EQUIV", true),
        ("", "to", true),
        ("", "content", true),
        ("a", "to", false),
        ("div", "content", false),
        ("a", "href", false),
        ("a", "title", false),
        ("a", "o", false),
        ("a", "data-on", false),
        ("animate", "attributeName", false),
    ];

    #[test]
    fn script_attributes_are_held_in_any_case() {
        for &(tag, name, held) in HELD {
            assert_eq!(holds_script(tag, name).is_some(), held, "<{tag} {name}>");
        }
    }

    /// URLs as a value holds them, and whether they run script when
    /// followed. Every side checks them: `guard_url` once `escape`d,
    /// `scheme` as a template's own text, and the browser runtime as it
    /// sets an attribute, spreads one and follows a redirect.
    const URLS: &[(&str, bool)] = &[
        ("javascript:alert(1)", true),
        ("JavaScript:alert(1)", true),
        (" \u{1}javascript:x", true),
        ("\u{0}\u{1f}javascript:x", true),
        ("java\tscript:x", true),
        ("java\nscript:x", true),
        ("java\rscript:x", true),
        ("vbscript:x", true),
        ("VBSCRIPT:x", true),
        ("https://x.com/a?b=javascript:c", false),
        ("/javascript:x", false),
        ("./javascript:x", false),
        ("mailto:a@b.c", false),
        ("data:text/plain,x", false),
        ("javascript&#58;x", false),
        ("javascript&colon;x", false),
        ("don't:x", false),
        ("a&b", false),
        ("", false),
        ("#top", false),
        ("?q=javascript:x", false),
        ("1javascript:x", false),
        ("javascripts:x", false),
        ("xjavascript:x", false),
        ("ünïcödé:x", false),
    ];

    /// Attributes that hold no URL, beside `URL_ATTRS`.
    const PLAIN_ATTRS: &[&str] = &["title", "alt", "value", "id", "data-href", "srcset"];

    /// Text and what `escape` makes of it.
    const TEXTS: &[(&str, &str)] = &[
        ("", ""),
        (
            r#"<a href="x">Tom & 'Jerry'</a> ünïcödé"#,
            "&lt;a href=&quot;x&quot;&gt;Tom &amp; &#39;Jerry&#39;&lt;/a&gt; ünïcödé",
        ),
        // Both sides of the 16-byte chunk edges, and none at all.
        ("0123456789abcde<", "0123456789abcde&lt;"),
        ("0123456789abcdef<", "0123456789abcdef&lt;"),
        (
            "<123456789abcdef0123456789abcdef>",
            "&lt;123456789abcdef0123456789abcdef&gt;",
        ),
        (
            "ünïcödé ünïcödé ünïcödé & ünïcödé",
            "ünïcödé ünïcödé ünïcödé &amp; ünïcödé",
        ),
        (
            "nothing to escape in this long line at all",
            "nothing to escape in this long line at all",
        ),
        ("\u{2028}\0'\"", "\u{2028}\0&#39;&quot;"),
    ];

    #[test]
    fn escapes() {
        for (input, want) in TEXTS {
            let mut s = String::new();
            escape(&mut s, input);
            assert_eq!(s, *want, "{input}");
        }
    }

    /// Every ASCII character and some longer ones, at every place of texts
    /// 0 to 40 bytes long (whole chunks, the overlapping last 16, the tail),
    /// alone and with a `&` at the end, against a character at a time.
    #[test]
    fn escapes_as_a_char_loop_would() {
        let slow = |s: &str| -> String {
            s.chars()
                .map(|c| match c {
                    '&' => "&amp;".to_string(),
                    '<' => "&lt;".to_string(),
                    '>' => "&gt;".to_string(),
                    '"' => "&quot;".to_string(),
                    '\'' => "&#39;".to_string(),
                    c => c.to_string(),
                })
                .collect()
        };
        let chars = (0..128u8)
            .map(char::from)
            .chain(['é', '€', '😀', '\u{2028}']);
        for c in chars {
            for len in 0..=40 {
                for at in 0..len {
                    let mut s: String = (0..len).map(|k| if k == at { c } else { 'a' }).collect();
                    for _ in 0..2 {
                        let mut out = String::new();
                        escape(&mut out, &s);
                        assert_eq!(out, slow(&s), "{s:?}");
                        s.push('&');
                    }
                }
            }
        }
    }

    #[test]
    fn urls_that_run_script_are_blocked() {
        for &(url, script) in URLS {
            let mut s = String::from("<a href=\"");
            let start = s.len();
            escape(&mut s, url);
            let escaped = s[start..].to_string();
            guard_url(&mut s, start);
            let want = if script { BLOCKED } else { &escaped };
            assert_eq!(&s[start..], want, "{url:?}");
            // A template's own text, decided before any expression.
            let want = if script {
                Scheme::Script
            } else {
                Scheme::Fixed
            };
            if !url.is_empty() && url.contains(':') {
                assert_eq!(scheme(&escaped), want, "{url:?}");
            }
        }
        // Character references a browser decodes, which `escape` never
        // writes: as they stand in a page, they may hide `javascript:`.
        for url in [
            "javascript&#58;x",
            "javascript&colon;x",
            "&#106;avascript:x",
        ] {
            assert!(runs_script(url), "{url}");
            assert_eq!(scheme(url), Scheme::Encoded);
        }
        // A start that leaves it to what comes next.
        for open in ["", " ", "java", "javascript", "java\nscript", "a1+-."] {
            assert_eq!(scheme(open), Scheme::Open, "{open:?}");
        }
        for fixed in ["/", "https:", "abcdefghijk", "a&amp;", "?", "1"] {
            assert_eq!(scheme(fixed), Scheme::Fixed, "{fixed:?}");
        }
        // Each name of the list, in any case, and no other: not one a
        // byte off, nor one the same length and first byte.
        for name in URL_ATTRS {
            assert!(
                is_url_attr(name) && is_url_attr(&name.to_ascii_uppercase()),
                "{name}"
            );
            for k in 0..name.len() {
                let mut other = name.as_bytes().to_vec();
                other[k] ^= 1;
                let other = String::from_utf8(other).unwrap();
                assert!(!is_url_attr(&other), "{other}");
            }
        }
        for name in PLAIN_ATTRS
            .iter()
            .chain(&["", "s", "srcs", "hre", "sty", "class"])
        {
            assert!(!is_url_attr(name), "{name}");
        }
    }

    /// Random URLs from pieces of hidden schemes, against how a browser
    /// reads one: leading spaces and controls dropped, tabs and newlines
    /// ignored, the scheme the letters before `:`. One that runs script is
    /// always blocked, and escaped text never holds a raw `<"'`.
    #[test]
    fn random_urls_that_run_script_are_blocked() {
        const PIECES: [&str; 18] = [
            "java", "JaVa", "script", "SCRIPT", "vb", ":", "\t", "\n", "\r", " ", "\u{1}", "&",
            "#58;", "x", "/", "<", "\"", "'",
        ];
        let browser_runs = |url: &str| {
            let s: String = url
                .trim_start_matches(|c: char| c <= ' ')
                .chars()
                .filter(|c| !matches!(c, '\t' | '\n' | '\r'))
                .collect();
            let Some((name, _)) = s.split_once(':') else {
                return false;
            };
            let name = name.to_ascii_lowercase();
            name == "javascript" || name == "vbscript"
        };
        let mut seed = 0x9e37_79b9_7f4a_7c15u64;
        let mut next = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        for _ in 0..50_000 {
            let n = next() % 8;
            let url: String = (0..n).map(|_| PIECES[(next() % 18) as usize]).collect();
            let mut s = String::from("<a href=\"");
            let start = s.len();
            escape(&mut s, &url);
            guard_url(&mut s, start);
            let out = &s[start..];
            if browser_runs(&url) {
                assert_eq!(out, BLOCKED, "{url:?}");
            }
            assert!(!out.contains(['<', '"', '\'']), "{url:?}");
        }
    }

    /// `s` as a JavaScript string literal.
    fn js(s: &str) -> String {
        let mut out = String::from("\"");
        for u in s.encode_utf16() {
            match u {
                0x20..0x7f if u != u16::from(b'"') && u != u16::from(b'\\') => {
                    out.push(char::from(u as u8));
                }
                _ => out.push_str(&format!("\\u{u:04x}")),
            }
        }
        out.push('"');
        out
    }

    /// The part of `src` from the line starting `from` to the line before
    /// the one starting `to`.
    fn slice<'a>(src: &'a str, from: &str, to: &str) -> &'a str {
        let a = src.find(from).unwrap_or_else(|| panic!("no {from:?}"));
        let b = src[a..].find(to).unwrap_or_else(|| panic!("no {to:?}")) + a;
        &src[a..b]
    }

    /// The case tables against the browser runtime: live.js's `attr` (which
    /// `:href` and `href={:x}` set by), extra.js's `{:...attrs}`, wisp.js's
    /// redirects. Run by Node when there is one; skipped quietly otherwise.
    #[test]
    fn the_browser_runtime_keeps_the_same_rules() {
        let live = crate::LIVE_JS.replace("\r\n", "\n");
        let extra = crate::EXTRA_JS.replace("\r\n", "\n");
        let wisp = crate::WISP_JS.replace("\r\n", "\n");
        let mut p = String::from("const X = {};\nconst out = [];\n");
        p.push_str(slice(&live, "const str = ", "// The root's listener"));
        p.push_str(slice(
            &live,
            "// A URL attribute whose value",
            "function binding(",
        ));
        p.push_str("let CUR;\n");
        p.push_str("const watch = (sc, a, L, f) => f(CUR, true);\n");
        p.push_str(slice(&extra, "const held = ", "\n};\n"));
        p.push_str("\n};\n");
        p.push_str(slice(&wisp, "  const script = ", "\n"));
        p.push_str(
            "\nconst enc = (s) => (s === undefined ? '-' : [...s].map((c) => c.codePointAt(0)).join(','));\n\
             const el = (localName) => { const e = { localName, at: {}, setAttribute: (n, v) => (e.at[n] = String(v)), \
             removeAttribute: (n) => delete e.at[n], addEventListener() {}, removeEventListener() {} }; return e; };\n\
             function check(name, url) {\n\
               const a = el(); attr(url, true, a, name);\n\
               const b = el(); CUR = { [name]: url }; X.spread(null, null, b, null, false, []);\n\
               out.push(enc(a.at[name]), enc(b.at[name]));\n\
             }\n",
        );
        let list = |p: &mut String, items: &mut dyn Iterator<Item = &str>| {
            p.push('[');
            for s in items {
                p.push_str(&js(s));
                p.push(',');
            }
            p.push(']');
        };
        let names: Vec<&str> = URL_ATTRS.iter().chain(PLAIN_ATTRS).copied().collect();
        p.push_str("for (const n of ");
        list(&mut p, &mut names.iter().copied());
        p.push_str(") for (const u of ");
        list(&mut p, &mut URLS.iter().map(|u| u.0));
        p.push_str(") check(n, u);\nfor (const u of ");
        list(&mut p, &mut URLS.iter().map(|u| u.0));
        p.push_str(") out.push(String(script(new URL(u, 'https://a.b/'))));\n");
        for &(tag, name, _) in HELD {
            let (tag, name) = (js(tag), js(name));
            p.push_str(&format!(
                "{{ const e = el({tag}); CUR = {{ [{name}]: 'x' }}; \
                 X.spread(null, null, e, null, false, []); out.push(e.at[{name}] === undefined); }}\n"
            ));
        }
        p.push_str("console.log(out.join('\\n'));\n");
        let run = std::process::Command::new("node").args(["-e", &p]).output();
        let Ok(run) = run else {
            return; // no Node here
        };
        assert!(
            run.status.success(),
            "{}",
            String::from_utf8_lossy(&run.stderr)
        );
        let out = String::from_utf8(run.stdout).unwrap();
        let mut lines = out.lines();
        let enc = |s: &str| {
            let codes: Vec<String> = s.chars().map(|c| (c as u32).to_string()).collect();
            codes.join(",")
        };
        let tf = |b: bool| if b { "true" } else { "false" };
        for name in &names {
            for &(url, script) in URLS {
                let want = enc(if script && is_url_attr(name) {
                    BLOCKED
                } else {
                    url
                });
                assert_eq!(lines.next(), Some(&*want), "attr {name}={url:?}");
                assert_eq!(lines.next(), Some(&*want), "spread {name}={url:?}");
            }
        }
        for &(url, script) in URLS {
            assert_eq!(lines.next(), Some(tf(script)), "redirect {url:?}");
        }
        for &(tag, name, held) in HELD {
            assert_eq!(lines.next(), Some(tf(held)), "spread <{tag} {name}>");
        }
        assert_eq!(lines.next(), None);
    }
}
