//! Auto-imports: the names a route file, `---` block, layout, component,
//! `src/hooks.rs` or app module uses with no `use` line. Three sources, in
//! this order (the file's own items and `use` lines beat them all):
//!
//! 1. the `pub` items of the app's own modules: `src/*.rs`, and the
//!    modules `main.rs` declares (`mod util;`, nested `pub mod`s, `mod.rs`);
//! 2. `[package.metadata.wisp] auto = ["chrono::{Utc, DateTime}"]` of the
//!    app's Cargo.toml;
//! 3. a few std names ([`STD`]).
//!
//! Decided at build time per file, from its tokens: a file gets a `use`
//! only for a name its code says (not in a string, comment or attribute,
//! not after `.` or `::`), so an unused name costs nothing, and the
//! request path never sees any of it. Two app modules with the same `pub`
//! name are a build error where the name is used, naming both files.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// The std names every file may use with no `use`, and their paths.
pub const STD: [(&str, &str); 11] = [
    ("HashMap", "::std::collections::HashMap"),
    ("HashSet", "::std::collections::HashSet"),
    ("BTreeMap", "::std::collections::BTreeMap"),
    ("BTreeSet", "::std::collections::BTreeSet"),
    ("VecDeque", "::std::collections::VecDeque"),
    ("Arc", "::std::sync::Arc"),
    ("Rc", "::std::rc::Rc"),
    ("Cow", "::std::borrow::Cow"),
    ("Duration", "::std::time::Duration"),
    ("Instant", "::std::time::Instant"),
    ("SystemTime", "::std::time::SystemTime"),
];

const KEYWORDS: &str = " as async await break const continue crate dyn else enum extern false fn for if impl in let loop match mod move mut pub ref return self Self static struct super trait true type unsafe use where while abstract become box do final macro override priv typeof unsized virtual yield try gen union macro_rules _ ";

fn keyword(s: &str) -> bool {
    s.len() < 13 && KEYWORDS.contains(&format!(" {s} "))
}

/// A `pub` item of an app module that files may use by name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Export {
    pub name: String,
    /// `fn`, `struct`, `enum`, `const`, `static`, `type`, `trait`, `union`.
    pub kind: &'static str,
    /// Its module's path: `["db"]`, `["util", "text"]`.
    pub segs: Vec<String>,
    /// Compiled by Wisp into `__mods` (`src/NAME.rs`), else a module of
    /// the crate's own (`crate::util`).
    pub wisp: bool,
    /// Its file from the project root, `/`-separated, and its line there.
    pub rel: String,
    pub line: u32,
    /// Its first line, for hovers: `pub fn slug(s: &str) -> String`.
    pub sig: String,
}

impl Export {
    /// Its path as a file at `base` writes it: `base` is how such a file
    /// reaches `__mods` (`super::__mods::` or `super::`).
    fn path(&self, base: &str) -> String {
        match self.wisp {
            true => format!("{base}{}::{}", self.segs.join("::"), self.name),
            false => format!("crate::{}::{}", self.segs.join("::"), self.name),
        }
    }

    /// How a file writes it without the auto-import: `db::Post`.
    pub fn qualified(&self) -> String {
        format!("{}::{}", self.segs.join("::"), self.name)
    }
}

/// What files may use by name, read once per build.
#[derive(Default)]
pub struct Auto {
    pub exports: Vec<Export>,
    /// `auto = [...]` of Cargo.toml: name and path.
    pub listed: Vec<(String, String)>,
}

/// One file's code to read names from: its text and its path from the
/// root. `rust`: Rust as written; else the code of a `.wisp` file's markup.
pub struct Src<'a> {
    pub text: &'a str,
    pub rel: &'a str,
}

impl Auto {
    /// The names of the app at `root`, whose Wisp-compiled modules are
    /// `wisp_mods` (name, file).
    pub fn load(root: &Path, wisp_mods: &[(String, PathBuf)]) -> Result<Auto, String> {
        let toml = std::fs::read_to_string(root.join("Cargo.toml")).unwrap_or_default();
        Ok(Auto {
            exports: exports(root, wisp_mods)?,
            listed: listed(&toml)?,
        })
    }

    /// The `use` paths a file needs, by name: what `used` names that
    /// `own` (the file's own code, and what the generated module adds,
    /// `extra`) does not define. `module`: the app module the file is,
    /// whose own items are not imported into it.
    pub fn uses(
        &self,
        used: &[Src],
        extra: &str,
        module: Option<&str>,
        base: &str,
    ) -> Result<Vec<(String, String)>, String> {
        let mut defined = Vec::new();
        let mut bound = Vec::new();
        let mut names: BTreeMap<&str, (bool, usize, u32)> = BTreeMap::new();
        let mut globs = false;
        let lexed: Vec<Vec<Tok>> = used.iter().map(|s| lex(s.text)).collect();
        for (k, toks) in lexed.iter().enumerate() {
            let d = defines(toks, 0);
            globs |= d.1;
            defined.extend(d.0);
            bound.extend(bindings(toks));
            for (n, line, code) in uses(toks) {
                let e = names.entry(n).or_insert((code, k, line));
                if code && !e.0 {
                    *e = (true, k, line);
                }
            }
        }
        let made = lex(extra);
        defined.extend(defines(&made, 1).0);
        let mut out = Vec::new();
        for (name, (code, k, line)) in names {
            if defined.contains(&name) {
                continue;
            }
            let mut hits: Vec<&Export> = (self.exports.iter())
                .filter(|e| {
                    e.name == name
                        && !(e.wisp && module == Some(e.segs[0].as_str()) && e.segs.len() == 1)
                })
                .collect();
            hits.dedup_by(|a, b| a.segs == b.segs);
            let is_bound = bound.contains(&name);
            match hits.as_slice() {
                [] => {}
                [e] => {
                    if !(is_bound && e.kind != "fn") {
                        out.push((name.to_string(), e.path(base)));
                    }
                    continue;
                }
                [a, b, ..] => {
                    if !code || is_bound {
                        continue;
                    }
                    return Err(format!(
                        "{}:{line}: `{name}` is `pub` in both {} and {}: write `{}` or `{}` there, or `use` the one you mean",
                        used[k].rel,
                        a.rel,
                        b.rel,
                        a.qualified(),
                        b.qualified(),
                    ));
                }
            }
            if globs {
                continue;
            }
            if let Some((_, p)) = self.listed.iter().find(|(n, _)| n == name) {
                out.push((name.to_string(), p.clone()));
            } else if let Some((_, p)) = STD.iter().find(|(n, _)| *n == name) {
                out.push((name.to_string(), p.to_string()));
            }
        }
        Ok(out)
    }
}

/// Per file (from the root), its auto-imports: name and path.
pub type FileImports = Vec<(String, Vec<(String, String)>)>;

/// The `use` lines for `imports`.
pub fn lines(imports: &[(String, String)]) -> String {
    let mut s = String::new();
    for (name, path) in imports {
        let tail = path.rsplit("::").next().unwrap_or(path);
        s.push_str("#[allow(unused_imports)] use ");
        s.push_str(path);
        if tail != name {
            s.push_str(" as ");
            s.push_str(name);
        }
        s.push_str(";\n");
    }
    s
}

/// The code of a `.wisp` file that names Rust: its `---` block and its
/// markup's `{…}` holes, outside `<script>` and `<style>`, the rest blanked
/// (newlines kept, so lines stay where they were).
pub fn wisp_code(src: &str) -> String {
    let b = src.as_bytes();
    let mut keep = vec![false; b.len()];
    let mut i = 0;
    // The `---` block, whole.
    let first = src.lines().find(|l| !l.trim().is_empty());
    if first.is_some_and(|l| l.trim() == "---") {
        let open = src.find("---").unwrap_or(0) + 3;
        let close = src[open..].find("\n---").map_or(src.len(), |c| open + c);
        keep[open..close].fill(true);
        i = (close + 4).min(b.len());
    }
    while i < b.len() {
        let rest = &b[i..];
        let tag = |t: &[u8]| rest.len() > t.len() && rest[..t.len()].eq_ignore_ascii_case(t);
        if tag(b"<script") || tag(b"<style") {
            let end: &[u8] = if tag(b"<script") {
                b"</script"
            } else {
                b"</style"
            };
            i += find(&b[i..], end).map_or(b.len() - i, |e| e + end.len());
        } else if rest.starts_with(b"<!--") {
            i += find(rest, b"-->").map_or(rest.len(), |e| e + 3);
        } else if b[i] == b'{' {
            let end = crate::template::hole_end(b, i + 1).unwrap_or(b.len());
            keep[i + 1..end].fill(true);
            i = end + 1;
        } else {
            i += 1;
        }
    }
    (src.char_indices())
        .map(|(k, c)| if keep[k] || c == '\n' { c } else { ' ' })
        .collect()
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len())
        .position(|w| w.eq_ignore_ascii_case(needle))
}

// ---- tokens ----

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Attr {
    /// `#[cfg(…)]`, `#[test]`: not always compiled.
    Cfg,
    /// `#[path = "…"]`: a module somewhere else.
    Path,
    /// `#[model]`: makes its struct `pub`.
    Model,
    Other,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Tok<'a> {
    /// A name and its line; `false`: inside a string's `{name}`.
    Id(&'a str, u32, bool),
    /// `::`
    Sep,
    P(u8),
    Attr(Attr),
}

/// The tokens of Rust `s` that matter here: names, `::`, punctuation and
/// attributes. Literals, comments, lifetimes and attribute contents are
/// dropped (a string's `{name}` stays, as a use). Never panics.
pub(crate) fn lex(s: &str) -> Vec<Tok<'_>> {
    let b = s.as_bytes();
    let mut out = Vec::new();
    let mut line = 1u32;
    let mut i = 0;
    // Inside an attribute: its bracket depth, and its kind.
    let mut attr: Option<(u32, Attr)> = None;
    let nl = |a: usize, z: usize| {
        b[a.min(b.len())..z.min(b.len())]
            .iter()
            .filter(|&&c| c == b'\n')
            .count() as u32
    };
    while i < b.len() {
        let c = b[i];
        if c == b'\n' {
            line += 1;
            i += 1;
        } else if c.is_ascii_whitespace() {
            i += 1;
        } else if b[i..].starts_with(b"//") {
            while i < b.len() && b[i] != b'\n' {
                i += 1;
            }
        } else if b[i..].starts_with(b"/*") {
            let e = (wisp_shared::rust::skip_block_comment(b, i) + 1).min(b.len());
            line += nl(i, e);
            i = e;
        } else if c == b'"' {
            let e = (wisp_shared::rust::skip_str(b, i) + 1).min(b.len());
            if attr.is_none() {
                fmt_names(s, i + 1, e.saturating_sub(1), line, &mut out);
            }
            line += nl(i, e);
            i = e;
        } else if c == b'\'' {
            let e = wisp_shared::rust::skip_char(b, i);
            if e > i {
                i = e + 1;
            } else {
                // A lifetime or a label: `'a`.
                i += 1;
                while i < b.len() && wisp_shared::rust::is_word(b[i]) {
                    i += 1;
                }
            }
        } else if c.is_ascii_digit() {
            while i < b.len()
                && (wisp_shared::rust::is_word(b[i])
                    || (b[i] == b'.' && b.get(i + 1).is_some_and(u8::is_ascii_digit)))
            {
                i += 1;
            }
        } else if c == b'_' || c.is_ascii_alphabetic() || c >= 0x80 {
            let start = i;
            while i < b.len() && (wisp_shared::rust::is_word(b[i]) || b[i] >= 0x80) {
                i += 1;
            }
            // Never split a char: `i` ends on ASCII or the end.
            let word = s.get(start..i).unwrap_or("");
            let next = b.get(i).copied();
            if matches!(word, "r" | "br" | "cr") && matches!(next, Some(b'"' | b'#')) {
                if let Some(h) = raw_hashes(b, i) {
                    let e = raw_end(b, i + h + 1, h);
                    line += nl(i, e);
                    i = e;
                    continue;
                }
                if word == "r" && next == Some(b'#') {
                    // `r#name`: the name.
                    let n = i + 1;
                    let mut j = n;
                    while j < b.len() && wisp_shared::rust::is_word(b[j]) {
                        j += 1;
                    }
                    if j > n && attr.is_none() {
                        out.push(Tok::Id(&s[n..j], line, true));
                    }
                    i = j.max(i + 1);
                    continue;
                }
            }
            if matches!(word, "b" | "c") && next == Some(b'"') {
                let e = (wisp_shared::rust::skip_str(b, i) + 1).min(b.len());
                line += nl(i, e);
                i = e;
                continue;
            }
            if word == "b" && next == Some(b'\'') {
                let e = wisp_shared::rust::skip_char(b, i);
                i = if e > i { e + 1 } else { i + 1 };
                continue;
            }
            if !word.is_empty() && attr.is_none() {
                out.push(Tok::Id(word, line, true));
            }
        } else if c == b'#' && attr.is_none() {
            // An attribute: `#[…]` or `#![…]`.
            let mut j = i + 1;
            if b.get(j) == Some(&b'!') {
                j += 1;
            }
            j = wisp_shared::rust::skip_space(b, j);
            if b.get(j) == Some(&b'[') {
                let k = wisp_shared::rust::skip_space(b, j + 1);
                let mut e = k;
                while e < b.len() && wisp_shared::rust::is_word(b[e]) {
                    e += 1;
                }
                let kind = match &b[k..e] {
                    b"cfg" | b"test" => Attr::Cfg,
                    b"path" => Attr::Path,
                    b"model" => Attr::Model,
                    _ => Attr::Other,
                };
                line += nl(i, j);
                attr = Some((1, kind));
                i = j + 1;
            } else {
                out.push(Tok::P(b'#'));
                i += 1;
            }
        } else {
            if let Some((d, kind)) = &mut attr {
                match c {
                    b'[' => *d += 1,
                    b']' => {
                        *d -= 1;
                        if *d == 0 {
                            out.push(Tok::Attr(*kind));
                            attr = None;
                        }
                    }
                    _ => {}
                }
            } else if c == b':' && b.get(i + 1) == Some(&b':') {
                out.push(Tok::Sep);
                i += 2;
                continue;
            } else if c.is_ascii() {
                out.push(Tok::P(c));
            }
            i += 1;
        }
    }
    out
}

/// The `#`s of a raw string whose `r` ends before `i`, if one starts there.
fn raw_hashes(b: &[u8], i: usize) -> Option<usize> {
    let mut j = i;
    while b.get(j) == Some(&b'#') {
        j += 1;
    }
    (b.get(j) == Some(&b'"')).then_some(j - i)
}

/// Just past the raw string whose text starts at `i`, closed by `"` and
/// `hashes` `#`s.
fn raw_end(b: &[u8], mut i: usize, hashes: usize) -> usize {
    while i < b.len() {
        if b[i] == b'"'
            && b[i + 1..]
                .iter()
                .take(hashes)
                .filter(|&&c| c == b'#')
                .count()
                == hashes
        {
            return (i + 1 + hashes).min(b.len());
        }
        i += 1;
    }
    b.len()
}

/// The `{name}` and `{name:…}` of a string (format arguments), as uses.
fn fmt_names<'a>(s: &'a str, a: usize, z: usize, line: u32, out: &mut Vec<Tok<'a>>) {
    let Some(text) = s.get(a..z) else { return };
    let b = text.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'{' {
            if b.get(i + 1) == Some(&b'{') {
                i += 2;
                continue;
            }
            let n = i + 1;
            let mut j = n;
            while j < b.len() && wisp_shared::rust::is_word(b[j]) {
                j += 1;
            }
            if j > n && !b[n].is_ascii_digit() && matches!(b.get(j), Some(b'}' | b':')) {
                let at = line + b[..i].iter().filter(|&&c| c == b'\n').count() as u32;
                out.push(Tok::Id(&text[n..j], at, false));
            }
            i = j.max(i + 1);
        } else {
            i += 1;
        }
    }
}

/// The names `toks` uses: (name, line, in code rather than a string).
fn uses<'a>(toks: &[Tok<'a>]) -> Vec<(&'a str, u32, bool)> {
    let mut out = Vec::new();
    let mut in_use = false;
    for (k, t) in toks.iter().enumerate() {
        let Tok::Id(n, line, code) = *t else {
            if in_use && *t == Tok::P(b';') {
                in_use = false;
            }
            continue;
        };
        if !code {
            out.push((n, line, false));
            continue;
        }
        if n == "use" {
            in_use = true;
        }
        if in_use || keyword(n) {
            continue;
        }
        let prev = k.checked_sub(1).and_then(|p| toks.get(p)).copied();
        let next = toks.get(k + 1).copied();
        let after = toks.get(k + 2).copied();
        let item = matches!(
            prev,
            Some(Tok::Id(
                "fn" | "struct"
                    | "enum"
                    | "const"
                    | "static"
                    | "type"
                    | "trait"
                    | "mod"
                    | "union"
                    | "let"
                    | "mut"
                    | "ref",
                ..
            ))
        );
        let field = matches!(prev, Some(Tok::P(b'.') | Tok::Sep));
        let label = next == Some(Tok::P(b':'));
        let mac = next == Some(Tok::P(b'!')) && after != Some(Tok::P(b'='));
        if !(item || field || label || mac) {
            out.push((n, line, true));
        }
    }
    out
}

/// The names `toks` defines at brace depth `depth` (items and `use`
/// lines), and whether it has a `use …::*` of its own there.
fn defines<'a>(toks: &[Tok<'a>], depth: u32) -> (Vec<&'a str>, bool) {
    let (mut out, mut globs) = (Vec::new(), false);
    let mut d = 0u32;
    let mut k = 0;
    while k < toks.len() {
        match toks[k] {
            Tok::P(b'{') => d += 1,
            Tok::P(b'}') => d = d.saturating_sub(1),
            Tok::Id(n, _, true) if d == depth => match n {
                "fn" | "struct" | "enum" | "static" | "type" | "trait" | "mod" | "union"
                | "const" => {
                    let j =
                        k + usize::from(matches!(toks.get(k + 1), Some(Tok::Id("mut", ..)))) + 1;
                    // `const fn f`: the `fn` names it.
                    if let Some(Tok::Id(name, ..)) = toks.get(j)
                        && !matches!(*name, "fn" | "unsafe" | "async" | "extern")
                    {
                        out.push(*name);
                    }
                }
                "macro_rules" => {
                    if let (Some(Tok::P(b'!')), Some(Tok::Id(name, ..))) =
                        (toks.get(k + 1), toks.get(k + 2))
                    {
                        out.push(*name);
                    }
                }
                "use" => {
                    let end = (toks[k..].iter().position(|t| *t == Tok::P(b';')))
                        .map_or(toks.len(), |e| k + e);
                    globs |= use_names(&toks[k + 1..end], &mut out);
                    k = end;
                }
                _ => {}
            },
            _ => {}
        }
        k += 1;
    }
    // `extern crate x;`
    for w in toks.windows(3) {
        if let [Tok::Id("extern", ..), Tok::Id("crate", ..), Tok::Id(n, ..)] = w {
            out.push(n);
        }
    }
    (out, globs)
}

/// The names a `use` tree brings in (its leaves, or what `as` names them),
/// into `out`; whether it has a glob.
fn use_names<'a>(tree: &[Tok<'a>], out: &mut Vec<&'a str>) -> bool {
    let mut globs = false;
    let mut last: Option<&'a str> = None;
    let mut prev_seg: Option<&'a str> = None;
    let mut k = 0;
    let flush = |last: &mut Option<&'a str>, prev: Option<&'a str>, out: &mut Vec<&'a str>| {
        if let Some(n) = last.take() {
            out.push(if n == "self" { prev.unwrap_or(n) } else { n });
        }
    };
    let mut stack: Vec<Option<&'a str>> = Vec::new();
    while k < tree.len() {
        match tree[k] {
            Tok::Id("as", ..) => {
                if let Some(Tok::Id(n, ..)) = tree.get(k + 1) {
                    last = (*n != "_").then_some(*n);
                    k += 1;
                }
            }
            Tok::Id(n, ..) => {
                if last.is_some() {
                    prev_seg = last;
                }
                last = Some(n);
            }
            Tok::Sep => {
                prev_seg = last.take().or(prev_seg);
            }
            Tok::P(b'*') => {
                globs = true;
                last = None;
            }
            Tok::P(b'{') => {
                stack.push(prev_seg);
                last = None;
            }
            Tok::P(b',') => {
                flush(&mut last, prev_seg, out);
                prev_seg = stack.last().copied().flatten();
            }
            Tok::P(b'}') => {
                flush(&mut last, prev_seg, out);
                prev_seg = stack.pop().flatten();
            }
            _ => {}
        }
        k += 1;
    }
    flush(&mut last, prev_seg, out);
    globs
}

/// Names bound as variables: `let x`, `let mut x`, `for x`, `|x|`,
/// `(x: T)`, `ref x`, `x @`. A `const`, `static` or unit struct of that
/// name would make the binding a pattern, so such names are not imported.
fn bindings<'a>(toks: &[Tok<'a>]) -> Vec<&'a str> {
    let mut out = Vec::new();
    for (k, t) in toks.iter().enumerate() {
        let Tok::Id(n, _, true) = *t else { continue };
        if keyword(n) {
            continue;
        }
        let prev = k.checked_sub(1).map(|p| toks[p]);
        let next = toks.get(k + 1).copied();
        let bind = matches!(
            prev,
            Some(Tok::Id("let" | "mut" | "ref" | "for", ..) | Tok::P(b'|'))
        ) || (matches!(prev, Some(Tok::P(b'(' | b','))) && next == Some(Tok::P(b':')))
            || next == Some(Tok::P(b'@'));
        if bind {
            out.push(n);
        }
    }
    out
}

// ---- the app's modules ----

/// The `pub` items of the app's modules: the Wisp-compiled `wisp_mods`
/// and the modules `src/main.rs` declares (with what they declare `pub`).
pub fn exports(root: &Path, wisp_mods: &[(String, PathBuf)]) -> Result<Vec<Export>, String> {
    let src = root.join("src");
    let mut out = Vec::new();
    let rel = |f: &Path| {
        f.strip_prefix(root)
            .unwrap_or(f)
            .to_string_lossy()
            .replace('\\', "/")
    };
    for (name, file) in wisp_mods {
        let text = crate::read_source(file).map_err(|e| format!("{}: {e}", rel(file)))?;
        let toks = lex(&text);
        // Wisp compiles this file into the generated code, where rustc
        // would look for `mod x;`'s file beside that, not here.
        if let Some((child, _)) = mods(&toks, true).into_iter().next() {
            return Err(format!(
                "{}: `mod {child};` in a module Wisp compiles has no file to find; declare it in src/main.rs (`mod {child};`), or write it inline as `mod {child} {{ … }}`",
                rel(file)
            ));
        }
        let m = Module {
            text: &text,
            rel: rel(file),
            wisp: true,
        };
        m.items(&toks, std::slice::from_ref(name), 0, &mut out);
    }
    // The crate root the app is in: the file with `wisp::app!()`.
    let lib = crate::read_source(&src.join("lib.rs")).unwrap_or_default();
    let root_file = if lib.contains("app!") {
        "lib.rs"
    } else {
        "main.rs"
    };
    if let Ok(text) = crate::read_source(&src.join(root_file)) {
        let toks = lex(&text);
        for (name, _) in mods(&toks, true) {
            if let Some(file) = mod_file(&src, &name) {
                read_mod(root, &file, vec![name], &mut out, 0);
            }
        }
    }
    Ok(out)
}

/// `src/NAME.rs` or `src/NAME/mod.rs` under `dir`.
fn mod_file(dir: &Path, name: &str) -> Option<PathBuf> {
    let a = dir.join(format!("{name}.rs"));
    let b = dir.join(name).join("mod.rs");
    [a, b].into_iter().find(|f| f.is_file())
}

fn read_mod(root: &Path, file: &Path, segs: Vec<String>, out: &mut Vec<Export>, depth: u32) {
    if depth > 16 {
        return;
    }
    let Ok(text) = crate::read_source(file) else {
        return;
    };
    let rel = file
        .strip_prefix(root)
        .unwrap_or(file)
        .to_string_lossy()
        .replace('\\', "/");
    // Its children's files: next to a `mod.rs`, else in a folder of its name.
    let dir = match file.file_name().and_then(|n| n.to_str()) {
        Some("mod.rs") => file.parent().map(Path::to_path_buf),
        _ => file.parent().zip(file.file_stem()).map(|(p, s)| p.join(s)),
    };
    let toks = lex(&text);
    let m = Module {
        text: &text,
        rel,
        wisp: false,
    };
    m.items(&toks, &segs, 0, out);
    if let Some(dir) = dir {
        for (child, public) in mods(&toks, false) {
            if public && let Some(f) = mod_file(&dir, &child) {
                let mut s = segs.clone();
                s.push(child);
                read_mod(root, &f, s, out, depth + 1);
            }
        }
    }
}

/// The `mod NAME;` (file modules) at the top of `toks`, not `#[cfg]` or
/// `#[path]`, and whether each is `pub` (or `any`: every one counts).
fn mods(toks: &[Tok], any: bool) -> Vec<(String, bool)> {
    let mut out = Vec::new();
    let mut d = 0u32;
    let mut skip = false;
    for (k, t) in toks.iter().enumerate() {
        match *t {
            Tok::P(b'{') => d += 1,
            Tok::P(b'}') => d = d.saturating_sub(1),
            Tok::Attr(a) if d == 0 => skip |= matches!(a, Attr::Cfg | Attr::Path),
            Tok::P(b';') if d == 0 => skip = false,
            Tok::Id("mod", ..) if d == 0 => {
                if let (Some(Tok::Id(name, ..)), Some(Tok::P(b';'))) =
                    (toks.get(k + 1), toks.get(k + 2))
                {
                    let public = vis(toks, k) != Vis::Private;
                    if !skip && (any || public) && !keyword(name) {
                        out.push((name.to_string(), public));
                    }
                }
                skip = false;
            }
            _ => {}
        }
    }
    out
}

#[derive(PartialEq, Eq)]
enum Vis {
    /// `pub` or `pub(crate)`: anyone in the crate sees it.
    Pub,
    /// `pub(super)`, `pub(in …)`.
    Some,
    Private,
}

/// The visibility of the item whose keyword is `toks[k]`, read backwards
/// over its qualifiers.
fn vis(toks: &[Tok], mut k: usize) -> Vis {
    while k > 0 {
        k -= 1;
        match toks[k] {
            Tok::Id("async" | "const" | "unsafe" | "extern" | "default", ..) => {}
            Tok::Id("pub", ..) => return Vis::Pub,
            Tok::P(b')') => {
                let open = toks[..k].iter().rposition(|t| *t == Tok::P(b'('));
                return match open {
                    Some(o) if o > 0 && matches!(toks[o - 1], Tok::Id("pub", ..)) => {
                        match toks.get(o + 1) {
                            Some(Tok::Id("crate", ..)) if o + 2 == k => Vis::Pub,
                            _ => Vis::Some,
                        }
                    }
                    _ => Vis::Private,
                };
            }
            _ => return Vis::Private,
        }
    }
    Vis::Private
}

struct Module<'a> {
    text: &'a str,
    rel: String,
    wisp: bool,
}

impl Module<'_> {
    /// The `pub` items of `toks` (a module's body), at `segs`; inline
    /// `pub mod`s too.
    fn items(&self, toks: &[Tok], segs: &[String], depth: u32, out: &mut Vec<Export>) {
        if depth > 16 {
            return;
        }
        let mut d = 0u32;
        let (mut cfg, mut model) = (false, false);
        let mut k = 0;
        while k < toks.len() {
            match toks[k] {
                Tok::P(b'{') => d += 1,
                Tok::P(b'}') => d = d.saturating_sub(1),
                Tok::Attr(a) if d == 0 => {
                    cfg |= a == Attr::Cfg;
                    model |= a == Attr::Model;
                }
                Tok::P(b';') if d == 0 => (cfg, model) = (false, false),
                Tok::Id(kw, ..) if d == 0 => {
                    let kind = match kw {
                        "fn" => "fn",
                        "struct" => "struct",
                        "enum" => "enum",
                        "static" => "static",
                        "type" => "type",
                        "trait" => "trait",
                        "union" => "union",
                        "mod" => "mod",
                        "const"
                            if !matches!(
                                toks.get(k + 1),
                                Some(Tok::Id("fn" | "unsafe" | "async" | "extern", ..))
                            ) =>
                        {
                            "const"
                        }
                        _ => {
                            k += 1;
                            continue;
                        }
                    };
                    let mut j = k + 1;
                    if matches!(toks.get(j), Some(Tok::Id("mut", ..))) {
                        j += 1;
                    }
                    let public = vis(toks, k) == Vis::Pub || std::mem::take(&mut model);
                    let skip = std::mem::take(&mut cfg);
                    let Some(Tok::Id(name, line, true)) = toks.get(j).copied() else {
                        k += 1;
                        continue;
                    };
                    if kind == "mod" {
                        if toks.get(j + 1) == Some(&Tok::P(b'{')) {
                            let end = close(toks, j + 1);
                            if public && !skip {
                                let mut s = segs.to_vec();
                                s.push(name.to_string());
                                self.items(&toks[j + 2..end], &s, depth + 1, out);
                            }
                            k = end + 1;
                            continue;
                        }
                    } else if public && !skip && name != "_" && name != "main" && !keyword(name) {
                        let sig = (self.text.lines().nth(line.saturating_sub(1) as usize))
                            .unwrap_or("")
                            .split('{')
                            .next()
                            .unwrap_or("")
                            .trim()
                            .trim_end_matches(';')
                            .to_string();
                        out.push(Export {
                            name: name.to_string(),
                            kind,
                            segs: segs.to_vec(),
                            wisp: self.wisp,
                            rel: self.rel.clone(),
                            line,
                            sig,
                        });
                    }
                }
                _ => {}
            }
            k += 1;
        }
    }
}

/// The index of the `}` closing the `{` at `open`, or the end.
fn close(toks: &[Tok], open: usize) -> usize {
    let mut d = 0u32;
    for (k, t) in toks.iter().enumerate().skip(open) {
        match t {
            Tok::P(b'{') => d += 1,
            Tok::P(b'}') => {
                d -= 1;
                if d == 0 {
                    return k;
                }
            }
            _ => {}
        }
    }
    toks.len().saturating_sub(1).max(open)
}

// ---- Cargo.toml ----

/// `auto = [...]` of `[package.metadata.wisp]`, checked: each entry a
/// path into a crate the app depends on (or std), naming items.
pub fn listed(toml: &str) -> Result<Vec<(String, String)>, String> {
    let deps = dependencies(toml);
    let mut out: Vec<(String, String)> = Vec::new();
    for e in crate::config::strings(toml, "auto")? {
        let bad = |m: &str| format!("Cargo.toml: auto = [\"{e}\"]: {m}");
        let e = e.trim();
        let (head, tail): (&str, Vec<&str>) = match e.split_once("::{") {
            Some((h, t)) => {
                let t = (t.trim_end().strip_suffix('}')).ok_or_else(|| bad("a `{` with no `}`"))?;
                (
                    h.trim(),
                    t.split(',')
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .collect(),
                )
            }
            None => match e.rsplit_once("::") {
                Some((h, t)) => (h.trim(), vec![t.trim()]),
                None => {
                    return Err(bad(
                        "a path to an item, like `uuid::Uuid` or `chrono::{Utc, DateTime}`",
                    ));
                }
            },
        };
        let mut segs = head.split("::").map(str::trim);
        let first = segs.next().unwrap_or("");
        let first = first.strip_prefix("::").unwrap_or(first);
        if !crate::ty::is_ident(first) || !segs.all(crate::ty::is_ident) {
            return Err(bad("each part of the path is a Rust name"));
        }
        if !matches!(first, "std" | "core" | "alloc") && !deps.iter().any(|d| d == first) {
            return Err(bad(&format!(
                "`{first}` is not in [dependencies]: `cargo add {}`",
                first.replace('_', "-")
            )));
        }
        if tail.is_empty() {
            return Err(bad("names no item"));
        }
        for item in tail {
            let words: Vec<&str> = item.split_whitespace().collect();
            let (path, name) = match words.as_slice() {
                [p] => (*p, *p),
                [p, "as", n] => (*p, *n),
                _ => return Err(bad(&format!("`{item}` is not `Name` or `Name as Other`"))),
            };
            if path == "*" {
                return Err(bad("a glob is not auto-imported: name each item"));
            }
            if !crate::ty::is_ident(path) || !crate::ty::is_ident(name) || keyword(name) {
                return Err(bad(&format!("`{item}` is not an item name")));
            }
            if out.iter().any(|(n, _)| n == name) {
                return Err(bad(&format!("`{name}` is listed twice")));
            }
            let head = head.trim_start_matches("::");
            out.push((name.to_string(), format!("::{head}::{path}")));
        }
    }
    Ok(out)
}

/// The crate names (as Rust writes them) of the app's dependencies.
fn dependencies(toml: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut on = false;
    for l in toml.lines() {
        let t = l.trim();
        if t.starts_with('[') {
            let h = t.trim_matches(['[', ']']).trim();
            on = h == "dependencies" || (h.starts_with("target.") && h.ends_with(".dependencies"));
            if let Some(name) = h.strip_prefix("dependencies.") {
                out.push(name.trim_matches('"').replace('-', "_"));
            }
            continue;
        }
        if on && let Some((k, v)) = t.split_once('=') {
            let k = k.trim().trim_matches('"');
            // `name = { package = "real" }` is still `name` in Rust.
            let _ = v;
            if !k.is_empty() && !k.starts_with('#') {
                out.push(k.replace('-', "_"));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn used(src: &str) -> Vec<&str> {
        uses(&lex(src)).into_iter().map(|u| u.0).collect()
    }

    fn auto(exports: &[(&str, &str, &str, &str)]) -> Auto {
        Auto {
            exports: (exports.iter())
                .map(|(name, kind, m, rel)| Export {
                    name: name.to_string(),
                    kind: match *kind {
                        "fn" => "fn",
                        "const" => "const",
                        _ => "struct",
                    },
                    segs: vec![m.to_string()],
                    wisp: true,
                    rel: rel.to_string(),
                    line: 1,
                    sig: String::new(),
                })
                .collect(),
            listed: vec![("Utc".into(), "::chrono::Utc".into())],
        }
    }

    fn names(a: &Auto, src: &str) -> Result<Vec<String>, String> {
        let s = [Src {
            text: src,
            rel: "src/routes/+page.rs",
        }];
        Ok((a.uses(&s, "", None, "super::__mods::")?.into_iter())
            .map(|u| u.0)
            .collect())
    }

    #[test]
    fn names_are_tokens_not_text() {
        let src = r##"
            // Post in a comment
            /* Post /* nested */ Post */
            #[doc = "Post"] #[derive(Post)]
            fn f<'a>(x: &'a str) -> Duration {
                let s = "Post"; let r = r#"Post "#; let c = 'P'; let b = b'x';
                x.Post; x.post(); db::Slug; vec![Thing]; format!("{Tag} {{Nope}} {0}");
                Duration::from_secs(1)
            }
        "##;
        let u = used(src);
        assert!(
            !u.contains(&"Post") && !u.contains(&"post") && !u.contains(&"Slug"),
            "{u:?}"
        );
        assert!(
            !u.contains(&"Nope") && !u.contains(&"vec") && !u.contains(&"a"),
            "{u:?}"
        );
        assert!(
            u.contains(&"Duration") && u.contains(&"Thing") && u.contains(&"Tag"),
            "{u:?}"
        );
        assert!(u.contains(&"db"), "{u:?}");
    }

    #[test]
    fn use_trees_name_their_leaves() {
        let (d, globs) = defines(
            &lex("use a::{b::{C, D as E}, self as F, G}; use h::*; use std::fmt::{self};"),
            0,
        );
        assert_eq!(d, ["C", "E", "F", "G", "fmt"]);
        assert!(globs);
        let src =
            "pub const fn k() {} const X: u8 = 1; static mut Y: u8 = 0; fn z() { struct In; }";
        assert_eq!(defines(&lex(src), 0).0, ["k", "X", "Y", "z"]);
    }

    #[test]
    fn local_and_explicit_names_win() {
        let a = auto(&[
            ("Post", "struct", "db", "src/db.rs"),
            ("slug", "fn", "text", "src/text.rs"),
        ]);
        let none = Vec::<String>::new();
        assert_eq!(
            names(&a, "fn f() -> Post { slug(1) }").unwrap(),
            ["Post", "slug"]
        );
        assert_eq!(
            names(&a, "struct Post; fn f() -> Post { slug(1) }").unwrap(),
            ["slug"]
        );
        assert_eq!(
            names(&a, "use other::slug; fn f() { slug(1) }").unwrap(),
            none
        );
        // std and the Cargo.toml list, unless the file has a glob of its own.
        let both = names(&a, "fn f(m: HashMap<u8, u8>, t: Utc) {}").unwrap();
        assert_eq!(both, ["HashMap", "Utc"]);
        let glob = names(&a, "use x::*; fn f(m: HashMap<u8, u8>) -> Post {}").unwrap();
        assert_eq!(glob, ["Post"]);
    }

    #[test]
    fn two_modules_with_a_name_is_an_error_where_it_is_used() {
        let a = auto(&[
            ("Post", "struct", "db", "src/db.rs"),
            ("Post", "struct", "blog", "src/blog.rs"),
        ]);
        let e = names(&a, "fn f() {\n    Post::new()\n}").unwrap_err();
        assert!(e.starts_with("src/routes/+page.rs:2: `Post`"), "{e}");
        assert!(e.contains("src/db.rs") && e.contains("src/blog.rs"), "{e}");
        assert!(e.contains("`db::Post`"), "{e}");
        assert!(
            names(&a, "fn f() { db::Post::new(); \"{Post}\"; }")
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn a_binding_keeps_a_const_out() {
        let a = auto(&[
            ("MAX", "const", "db", "src/db.rs"),
            ("count", "fn", "db", "src/db.rs"),
        ]);
        let n = names(&a, "fn f() { let MAX = 1; let count = count(); }").unwrap();
        assert_eq!(n, ["count"]);
        assert!(names(&a, "fn f(MAX: u8) {}").unwrap().is_empty());
    }

    #[test]
    fn a_module_does_not_import_itself() {
        let a = auto(&[("Post", "struct", "db", "src/db.rs")]);
        let s = [Src {
            text: "fn f() -> Post {}",
            rel: "src/db.rs",
        }];
        assert!(a.uses(&s, "", Some("db"), "super::").unwrap().is_empty());
        let ok = a.uses(&s, "", Some("blog"), "super::").unwrap();
        assert_eq!(ok, [("Post".to_string(), "super::db::Post".to_string())]);
        assert_eq!(
            lines(&ok),
            "#[allow(unused_imports)] use super::db::Post;\n"
        );
    }

    #[test]
    fn the_cargo_list_is_checked() {
        let toml = |auto: &str| {
            format!(
                "[package]\nname = \"a\"\n[dependencies]\nchrono = \"0.4\"\nuuid-x = {{ version = \"1\" }}\n[package.metadata.wisp]\nauto = [{auto}]\n"
            )
        };
        let ok = listed(&toml(
            r#""chrono::{Utc, DateTime as Dt}", "uuid_x::Uuid", "std::fmt::Write""#,
        ))
        .unwrap();
        let n: Vec<&str> = ok.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(n, ["Utc", "Dt", "Uuid", "Write"]);
        assert_eq!(ok[1].1, "::chrono::DateTime");
        for (bad, says) in [
            (r#""serde::Serialize""#, "not in [dependencies]"),
            (r#""chrono::*""#, "glob"),
            (r#""chrono::{*}""#, "glob"),
            (r#""chrono""#, "a path to an item"),
            (r#""chrono::{Utc""#, "no `}`"),
            (r#""chrono::{Utc, Utc}""#, "twice"),
            (r#""chrono::fn""#, "not an item name"),
            (r#""chrono::{A B}""#, "`Name as Other`"),
        ] {
            let e = listed(&toml(bad)).unwrap_err();
            assert!(e.contains(says), "{bad}: {e}");
        }
    }

    #[test]
    fn markup_code_is_its_holes() {
        let src = "---\nlet a = one();\n---\n<p title=\"{two()}\">Post {three()}</p>\n<script>let x = {four}</script>\n<!-- {five} -->";
        let code = wisp_code(src);
        assert_eq!(code.lines().count(), src.lines().count());
        assert_eq!(used(&code), ["one", "two", "three"], "{code}");
    }

    #[test]
    fn modules_the_crate_root_declares_are_read() {
        let root = std::env::temp_dir().join(format!("wisp-auto-mods-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let files = [
            (
                "src/main.rs",
                "mod util;\n#[cfg(test)]\nmod tests;\nmod gone;\nwisp::app!();",
            ),
            ("src/tests.rs", "pub fn t() {}"),
            (
                "src/util/mod.rs",
                "pub mod text;\nmod hidden;\npub(crate) fn top() {}\npub(super) fn no() {}\npub mod inner { pub struct In; }",
            ),
            (
                "src/util/text.rs",
                "/// Slugs.\npub fn slug(s: &str) -> String { s.into() }\n#[cfg(test)]\npub fn only_test() {}",
            ),
            ("src/util/hidden.rs", "pub fn secret() {}"),
            (
                "src/db.rs",
                "#[model]\nstruct Post { t: String }\npub static POSTS: Table<Post> = Table::saved();\nfn private() {}",
            ),
        ];
        for (p, c) in files {
            let f = root.join(p);
            std::fs::create_dir_all(f.parent().unwrap()).unwrap();
            std::fs::write(f, c).unwrap();
        }
        let e = exports(&root, &[("db".into(), root.join("src/db.rs"))]).unwrap();
        let _ = std::fs::remove_dir_all(&root);
        let got: Vec<String> = (e.iter())
            .map(|e| format!("{} {} {}", e.path("M::"), e.rel, e.line))
            .collect();
        assert_eq!(
            got,
            [
                "M::db::Post src/db.rs 2",
                "M::db::POSTS src/db.rs 3",
                "crate::util::top src/util/mod.rs 3",
                "crate::util::inner::In src/util/mod.rs 5",
                "crate::util::text::slug src/util/text.rs 2",
            ]
        );
        assert_eq!(e[4].sig, "pub fn slug(s: &str) -> String");
    }

    #[test]
    fn a_child_file_of_a_wisp_module_is_an_error() {
        let root = std::env::temp_dir().join(format!("wisp-auto-child-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/db.rs"), "mod rows;\npub fn f() {}").unwrap();
        let e = exports(&root, &[("db".into(), root.join("src/db.rs"))]);
        let _ = std::fs::remove_dir_all(&root);
        assert!(e.unwrap_err().contains("src/db.rs: `mod rows;`"));
    }

    /// Random Rust-ish text never panics a reader, and a name only in a
    /// comment or string is never a use.
    #[test]
    fn fuzz_never_panics() {
        const BITS: &[&str] = &[
            "fn ",
            "pub ",
            "use ",
            "::",
            ":",
            "{",
            "}",
            "(",
            ")",
            "[",
            "]",
            "#",
            "#![",
            "!",
            "'",
            "'a",
            "\"",
            "r#\"",
            "\"#",
            "b'",
            "//",
            "/*",
            "*/",
            "\n",
            " ",
            "Post",
            "x",
            "é",
            "€",
            "let ",
            "mut ",
            "|",
            ",",
            ";",
            ".",
            "*",
            "as ",
            "mod ",
            "1.5",
            "{x}",
            "{{",
            "<script>",
            "---\n",
            "r#",
            "br\"",
            "\\",
            "@",
            "<!--",
            "-->",
            "pub(crate) ",
            "#[model]",
            "#[cfg(x)]",
        ];
        let mut rng = wisp_shared::rng::Rng::new(7);
        let a = auto(&[
            ("Post", "struct", "db", "src/db.rs"),
            ("Post", "struct", "b", "src/b.rs"),
        ]);
        for _ in 0..5000 {
            let n = rng.below(40);
            let s: String = (0..n).map(|_| BITS[rng.below(BITS.len())]).collect();
            let toks = lex(&s);
            let _ = (
                uses(&toks),
                defines(&toks, 0),
                defines(&toks, 1),
                bindings(&toks),
            );
            let _ = (wisp_code(&s), names(&a, &s), mods(&toks, true));
            let m = Module {
                text: &s,
                rel: String::new(),
                wisp: true,
            };
            m.items(&toks, &["m".into()], 0, &mut Vec::new());
            let plain = s.replace(['"', '/', '*', '\'', '#', '\\', '\n'], " ");
            let quiet = format!("{plain}\n// Zed\n\"Zed\" /* Zed */");
            assert!(!used(&quiet).contains(&"Zed"), "{quiet}");
        }
    }
}
