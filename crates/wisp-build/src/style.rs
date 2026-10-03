//! Scoped styles: a bare `<style>` in a page, layout or component applies
//! to that file's elements alone. Every element the file writes gets its
//! class (`.w-` and a hash of the file's path, see `class`), and each
//! selector here gets the class on its last compound that is not
//! `:global(…)`: `.card p` → `.card p.w-x`, `p::before` → `p.w-x::before`.
//!
//! The rewriter keeps the CSS as written but for those classes and its
//! comments. It reads at-rules that hold rules (`@media`, `@supports`,
//! `@container`, `@layer`, …) and nested rules (`&`); `@keyframes`,
//! `@font-face` and the like are copied, their names global.

use crate::fnv1a;

/// The class of the file at `rel` (project-relative, `/`-separated): `w-`
/// and six letters or digits, about 31 bits of its path's hash.
pub fn class(rel: &str) -> String {
    const DIGITS: &[u8; 36] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    let mut h = fnv1a(rel.as_bytes());
    let mut out = String::from("w-");
    for _ in 0..6 {
        out.push(DIGITS[(h % 36) as usize] as char);
        h /= 36;
    }
    out
}

/// `css` with each selector scoped by `class`, or what is wrong and its
/// byte offset in `css`.
pub fn scope(css: &str, class: &str) -> Result<String, (usize, String)> {
    let (src, cuts) = strip_comments(css);
    let mut s = Scoper {
        src: &src,
        b: src.as_bytes(),
        i: 0,
        out: String::with_capacity(css.len() + 64),
        class,
    };
    let at = |i: usize| {
        i + cuts
            .iter()
            .take_while(|c| c.0 <= i)
            .last()
            .map_or(0, |c| c.1)
    };
    s.rules(false).map_err(|(i, msg)| (at(i), msg))?;
    if s.i < s.b.len() {
        return Err((at(s.i), "this `}` closes nothing".into()));
    }
    Ok(s.out.trim().to_string())
}

/// `css` without its comments, each one a space, and where they were: per
/// comment, its space's end in the result and the bytes left out up to it.
fn strip_comments(css: &str) -> (String, Vec<(usize, usize)>) {
    let b = css.as_bytes();
    let mut out = String::with_capacity(css.len());
    // (offset in out, how many bytes of css were dropped before it)
    let mut cuts = Vec::new();
    let (mut i, mut from, mut dropped) = (0, 0, 0);
    while i < b.len() {
        match b[i] {
            b'"' | b'\'' => i = string_end(b, i),
            b'\\' => i += 2,
            b'/' if b.get(i + 1) == Some(&b'*') => {
                let end = css[i + 2..].find("*/").map_or(b.len(), |n| i + 2 + n + 2);
                out.push_str(&css[from..i]);
                out.push(' ');
                dropped += end - i - 1;
                cuts.push((out.len(), dropped));
                i = end;
                from = end;
            }
            _ => i += 1,
        }
    }
    out.push_str(&css[from.min(css.len())..]);
    (out, cuts)
}

/// Past the string that starts at `i` (its quote), or the end.
fn string_end(b: &[u8], mut i: usize) -> usize {
    let q = b[i];
    i += 1;
    while i < b.len() && b[i] != q && b[i] != b'\n' {
        i += if b[i] == b'\\' { 2 } else { 1 };
    }
    (i + 1).min(b.len())
}

struct Scoper<'a> {
    src: &'a str,
    b: &'a [u8],
    i: usize,
    out: String,
    class: &'a str,
}

/// At-rules whose block is declarations or keyframes, copied as they are.
const OPAQUE: [&str; 7] = [
    "font-face",
    "page",
    "property",
    "counter-style",
    "font-feature-values",
    "view-transition",
    "position-try",
];

impl Scoper<'_> {
    /// Rules (with declarations too in a style rule's block, `nested`)
    /// until a `}` or the end, which is left for the caller.
    fn rules(&mut self, nested: bool) -> Result<(), (usize, String)> {
        loop {
            let ws = self.i;
            while self.i < self.b.len() && self.b[self.i].is_ascii_whitespace() {
                self.i += 1;
            }
            self.out.push_str(&self.src[ws..self.i]);
            if self.i >= self.b.len() || self.b[self.i] == b'}' {
                return Ok(());
            }
            let start = self.i;
            if self.b[start] == b'@' {
                self.at_rule(nested)?;
                continue;
            }
            // A custom property's value may hold braces.
            let custom = nested && self.src[start..].starts_with("--");
            let (end, stop) = self.until(start, custom);
            match stop {
                b'{' if !custom => {
                    let prelude = &self.src[start..end];
                    let scoped = self.selectors(prelude).map_err(|e| (start, e))?;
                    self.out.push_str(&scoped);
                    self.out.push('{');
                    self.i = end + 1;
                    self.rules(true)?;
                    self.close(start)?;
                }
                // A declaration, or (outside a block) a stray one, as is.
                b';' => {
                    self.out.push_str(&self.src[start..=end]);
                    self.i = end + 1;
                }
                _ => {
                    self.out.push_str(&self.src[start..end]);
                    self.i = end;
                }
            }
        }
    }

    /// `@name prelude;` or `@name prelude { … }` at `self.i`.
    fn at_rule(&mut self, nested: bool) -> Result<(), (usize, String)> {
        let start = self.i;
        let name_end = (start + 1..self.b.len())
            .find(|&k| !(self.b[k].is_ascii_alphanumeric() || self.b[k] == b'-'))
            .unwrap_or(self.b.len());
        let name = self.src[start + 1..name_end].to_ascii_lowercase();
        if name == "import" {
            return Err((
                start,
                "@import goes in src/app.css: a scoped <style> is added after it".into(),
            ));
        }
        let (end, stop) = self.until(name_end, false);
        self.out.push_str(&self.src[start..end]);
        self.i = end;
        match stop {
            b';' => {
                self.out.push(';');
                self.i += 1;
            }
            b'{' => {
                let opaque = name.ends_with("keyframes") || OPAQUE.contains(&name.as_str());
                if opaque {
                    let close = self.block_end(end)?;
                    self.out.push_str(&self.src[end..=close]);
                    self.i = close + 1;
                } else {
                    self.out.push('{');
                    self.i += 1;
                    self.rules(nested)?;
                    self.close(start)?;
                }
            }
            _ => {}
        }
        Ok(())
    }

    /// The `}` of the block opened at `open`'s rule, at `self.i`.
    fn close(&mut self, open: usize) -> Result<(), (usize, String)> {
        if self.b.get(self.i) != Some(&b'}') {
            return Err((open, "this rule's `{` is never closed".into()));
        }
        self.out.push('}');
        self.i += 1;
        Ok(())
    }

    /// From `i`, the first `{`, `;` or `}` outside strings, parentheses and
    /// brackets (with `braces`, outside nested braces too, and never `{`),
    /// and which it is (0 at the end).
    fn until(&self, mut i: usize, braces: bool) -> (usize, u8) {
        let b = self.b;
        let mut depth = 0usize;
        while i < b.len() {
            match b[i] {
                b'"' | b'\'' => {
                    i = string_end(b, i);
                    continue;
                }
                b'\\' => i += 1,
                b'(' | b'[' => depth += 1,
                b')' | b']' => depth = depth.saturating_sub(1),
                b'{' if braces => depth += 1,
                b'}' if braces && depth > 0 => depth -= 1,
                c @ (b'{' | b';' | b'}') if depth == 0 => return (i, c),
                _ => {}
            }
            i += 1;
        }
        (b.len(), 0)
    }

    /// The `}` that closes the `{` at `open`.
    fn block_end(&self, open: usize) -> Result<usize, (usize, String)> {
        let mut i = open + 1;
        loop {
            match self.until(i, true) {
                (end, b';') => i = end + 1,
                (end, b'}') => return Ok(end),
                _ => return Err((open, "this `{` is never closed".into())),
            }
        }
    }

    /// A selector list, each selector scoped.
    fn selectors(&self, list: &str) -> Result<String, String> {
        let mut out = String::with_capacity(list.len() + 16);
        for (k, sel) in split_top(list, b',').into_iter().enumerate() {
            if k > 0 {
                out.push(',');
            }
            out.push_str(&scope_selector(sel, self.class)?);
        }
        Ok(out)
    }
}

/// `s` split at each `sep` outside strings, parentheses and brackets.
fn split_top(s: &str, sep: u8) -> Vec<&str> {
    let b = s.as_bytes();
    let (mut parts, mut depth, mut from, mut i) = (Vec::new(), 0usize, 0, 0);
    while i < b.len() {
        match b[i] {
            b'"' | b'\'' => {
                i = string_end(b, i);
                continue;
            }
            b'\\' => i += 1,
            b'(' | b'[' => depth += 1,
            b')' | b']' => depth = depth.saturating_sub(1),
            c if c == sep && depth == 0 => {
                parts.push(&s[from..i]);
                from = i + 1;
            }
            _ => {}
        }
        i += 1;
    }
    parts.push(&s[from.min(s.len())..]);
    parts
}

/// One selector: its compounds (split at combinators), the class on the
/// last that is not wholly `:global(…)` (none when that one refers to the
/// rule around it with `&`, which is scoped already), and every
/// `:global(x)` replaced by `x`.
fn scope_selector(sel: &str, class: &str) -> Result<String, String> {
    let b = sel.as_bytes();
    // Each compound's range.
    let mut compounds: Vec<(usize, usize)> = Vec::new();
    let (mut i, mut depth, mut start) = (0, 0usize, None::<usize>);
    while i <= b.len() {
        let c = b.get(i).copied().unwrap_or(b' ');
        let combinator = depth == 0 && (c.is_ascii_whitespace() || matches!(c, b'>' | b'+' | b'~'));
        if combinator {
            if let Some(s) = start.take() {
                compounds.push((s, i));
            }
            i += 1;
            continue;
        }
        start.get_or_insert(i);
        match c {
            b'"' | b'\'' => {
                i = string_end(b, i);
                continue;
            }
            b'\\' => i += 1,
            b'(' | b'[' => depth += 1,
            b')' | b']' => depth = depth.saturating_sub(1),
            _ => {}
        }
        i += 1;
    }
    let global = |&(s, e): &(usize, usize)| sel[s..e].starts_with(":global(");
    let target = compounds.iter().rev().find(|c| !global(c)).copied();
    let mut out = String::with_capacity(sel.len() + class.len() + 1);
    let mut from = 0;
    if let Some((s, e)) = target.filter(|&(s, e)| !sel[s..e].contains('&')) {
        let at = s + first_pseudo(&sel[s..e]).unwrap_or(e - s);
        out.push_str(&unglobal(&sel[..at])?);
        out.push('.');
        out.push_str(class);
        from = at;
    }
    out.push_str(&unglobal(&sel[from..])?);
    Ok(out)
}

/// Where a compound's first pseudo-class or pseudo-element begins, outside
/// parentheses and escapes: the class goes before it.
fn first_pseudo(compound: &str) -> Option<usize> {
    let b = compound.as_bytes();
    let (mut i, mut depth) = (0, 0usize);
    while i < b.len() {
        match b[i] {
            b'\\' => i += 1,
            b'(' | b'[' => depth += 1,
            b')' | b']' => depth = depth.saturating_sub(1),
            b':' if depth == 0 => return Some(i),
            _ => {}
        }
        i += 1;
    }
    None
}

/// `s` with each `:global(x)` as `x`.
fn unglobal(s: &str) -> Result<String, String> {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(at) = rest.find(":global") {
        out.push_str(&rest[..at]);
        let after = &rest[at + 7..];
        if !after.starts_with('(') {
            return Err("write :global(selector), with the selector in parentheses".into());
        }
        let b = after.as_bytes();
        let (mut depth, mut k) = (0usize, 0);
        let close = loop {
            match b.get(k) {
                None => return Err(":global( is never closed".into()),
                Some(b'(') => depth += 1,
                Some(b')') => {
                    depth -= 1;
                    if depth == 0 {
                        break k;
                    }
                }
                _ => {}
            }
            k += 1;
        };
        out.push_str(after[1..close].trim());
        rest = &after[close + 1..];
    }
    out.push_str(rest);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(css: &str) -> String {
        scope(css, "w-x").unwrap()
    }

    #[test]
    fn selectors_get_the_class_on_their_last_compound() {
        assert_eq!(s("h1 { color: red }"), "h1.w-x { color: red }");
        assert_eq!(s("h1, .a p {x:1}"), "h1.w-x, .a p.w-x {x:1}");
        assert_eq!(s("ul > li + li ~ a {}"), "ul > li + li ~ a.w-x {}");
        assert_eq!(s("ul>li{}"), "ul>li.w-x{}");
        assert_eq!(s("a:hover, p::before {}"), "a.w-x:hover, p.w-x::before {}");
        assert_eq!(s(":not(.a) {}"), ".w-x:not(.a) {}");
        assert_eq!(s("* {}"), "*.w-x {}");
        assert_eq!(s("a[href^='x:y'] {}"), "a[href^='x:y'].w-x {}");
        assert_eq!(s(".md\\:flex {}"), ".md\\:flex.w-x {}");
        assert_eq!(s("li:is(.a, .b) {}"), "li.w-x:is(.a, .b) {}");
    }

    #[test]
    fn global_opts_out() {
        assert_eq!(s(":global(body) {}"), "body {}");
        assert_eq!(s(":global(.dark) p {}"), ".dark p.w-x {}");
        assert_eq!(s("p :global(strong) {}"), "p.w-x strong {}");
        assert_eq!(s(":global(a):hover {}"), "a:hover {}");
        assert!(scope(":global .x {}", "w-x").is_err());
    }

    #[test]
    fn at_rules() {
        assert_eq!(
            s("@media (min-width: 1px) { a, b { x: 1 } }"),
            "@media (min-width: 1px) { a.w-x, b.w-x { x: 1 } }"
        );
        assert_eq!(
            s("@supports (display: grid) { @container (width > 1px) { p {} } }"),
            "@supports (display: grid) { @container (width > 1px) { p.w-x {} } }"
        );
        let frames = "@keyframes spin { from { a: 1 } to { a: 2 } }";
        assert_eq!(s(frames), frames);
        let face = "@font-face { font-family: x; src: url(a.woff) }";
        assert_eq!(s(face), face);
        assert_eq!(s("@layer a, b;\np{}"), "@layer a, b;\np.w-x{}");
        let (at, msg) = scope("p{}\n@import 'x.css';", "w-x").unwrap_err();
        assert_eq!(at, 4);
        assert!(msg.contains("src/app.css"));
    }

    #[test]
    fn nesting() {
        assert_eq!(
            s(".card { color: red; &:hover { x: 1 } h2 { y: 2 } > p { z: 3 } }"),
            ".card.w-x { color: red; &:hover { x: 1 } h2.w-x { y: 2 } > p.w-x { z: 3 } }"
        );
        assert_eq!(
            s("a { @media (x) { color: red; b { c: d } } }"),
            "a.w-x { @media (x) { color: red; b.w-x { c: d } } }"
        );
        assert_eq!(s("p { --x: { a: b }; }"), "p.w-x { --x: { a: b }; }");
    }

    #[test]
    fn comments_strings_and_errors() {
        assert_eq!(
            s("/* a, b { } */ p /* x */ { content: \"a{b}\" }"),
            "p.w-x   { content: \"a{b}\" }"
        );
        let (at, _) = scope("p {}\n.a { x: 1", "w-x").unwrap_err();
        assert_eq!(at, 5);
        let (at, _) = scope("/* long */ p {} }", "w-x").unwrap_err();
        assert_eq!(at, 16);
    }

    #[test]
    fn classes_are_short_and_stable() {
        let c = class("src/routes/+page.wisp");
        assert_eq!(c.len(), 8);
        assert_eq!(c, class("src/routes/+page.wisp"));
        assert_ne!(c, class("src/routes/about/+page.wisp"));
    }
}
