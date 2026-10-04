//! Translations: `src/locales/en.json`, `fr.json`, … read and checked
//! against each other (the same keys, the same placeholders), and each
//! `t("key", args)` of a template checked against them and compiled to an
//! index into a table, per locale, of the message as parts. A missing key,
//! an unknown one or a placeholder that differs is a build error, unless
//! `missing warn` (below) lets another locale fall back to the default's.
//!
//! The URLs' side is `i18n = [...]` in `[package.metadata.wisp]`, strings
//! of `name value`: `default fr` (the locale a request gets that names
//! none), `prefix always|as-needed|optional` (how `[[lang=locale]]` shows
//! in URLs, `wisp::Prefix`), `domain example.fr fr` (a host's locale, one
//! per line) and `missing warn|error` (a locale without a key the default
//! has: a warning and the default's message, or the build error).
//!
//! A message is text with ICU's `{name}` and `{count, plural, one {# item}
//! other {# items}}` (cases `=N` and the locale's CLDR ones, `other`
//! required; `#` is the count); `'{'` writes a brace, `''` an apostrophe.

use crate::json_str;
use std::fmt::Write as _;
use std::path::Path;
use wisp_shared::plural;
use wisp_shared::rust::{is_word, skip_literal, skip_space};

/// The app's locales, from `src/locales/*.json`.
pub struct Locales {
    /// The files' names, sorted: `en`, `fr`.
    pub names: Vec<String>,
    /// Each one's plural rule (see `wisp_shared::plural`).
    rules: Vec<u8>,
    /// Sorted by name.
    keys: Vec<Key>,
    /// The locale a request gets that names none, by index.
    pub default: usize,
    /// `wisp::Prefix` as a number: 0 optional, 1 always, 2 as-needed.
    pub prefix: u8,
    /// Each domain and its locale's index.
    pub domains: Vec<(String, usize)>,
    /// What `missing warn` let through: file, line and what.
    pub warnings: Vec<(String, u32, String)>,
}

/// `i18n = [...]` of the app's Cargo.toml.
#[derive(Default)]
struct Settings {
    default: Option<String>,
    prefix: u8,
    domains: Vec<(String, String)>,
    warn: bool,
}

/// The `Settings` of `toml`: each string `name value`, checked.
fn settings(toml: &str) -> Result<Settings, String> {
    let mut s = Settings::default();
    for e in crate::config::strings(toml, "i18n")? {
        let at = |m: &str| format!("Cargo.toml: i18n = [\"{e}\"]: {m}");
        let w: Vec<&str> = e.split_whitespace().collect();
        match w[..] {
            ["default", l] => s.default = Some(l.into()),
            ["prefix", "optional"] => s.prefix = 0,
            ["prefix", "always"] => s.prefix = 1,
            ["prefix", "as-needed"] => s.prefix = 2,
            ["prefix", _] => return Err(at("the prefix is optional, always or as-needed")),
            ["domain", host, l] => {
                let ok = host
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b".-:".contains(&b));
                if !ok {
                    return Err(at(
                        "a domain is a host name, `example.fr` or `localhost:3000`",
                    ));
                }
                s.domains.push((host.to_ascii_lowercase(), l.into()));
            }
            ["missing", "warn"] => s.warn = true,
            ["missing", "error"] => s.warn = false,
            ["missing", _] => return Err(at("missing is warn or error")),
            _ => {
                return Err(at(
                    "write `default fr`, `prefix always`, `domain example.fr fr` or `missing warn`",
                ));
            }
        }
    }
    Ok(s)
}

struct Key {
    name: String,
    /// Its placeholders, in the order the first locale names them, each
    /// with whether a plural counts by it.
    args: Vec<(String, bool)>,
    /// Its message in each locale.
    msgs: Vec<Vec<Part>>,
}

#[derive(Debug, PartialEq, Clone)]
enum Part {
    Text(String),
    Arg(String),
    Plural(String, Vec<(Case, Vec<Part>)>),
}

#[derive(Debug, PartialEq, Clone)]
enum Case {
    Is(u64),
    Cat(u8),
}

/// The locales of the app at `root`, or `None` without `src/locales`.
pub fn load(root: &Path) -> Result<Option<Locales>, String> {
    let dir = root.join("src").join("locales");
    let Ok(list) = std::fs::read_dir(&dir) else {
        return Ok(None);
    };
    let mut files: Vec<(String, String)> = Vec::new();
    for e in list.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        let Some(stem) = name.strip_suffix(".json") else {
            continue;
        };
        let rel = format!("src/locales/{name}");
        let ok = stem.starts_with(|c: char| c.is_ascii_alphabetic())
            && stem
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
        if !ok {
            return Err(format!(
                "{rel}: a locale file is named for its language: en.json, fr.json, pt-BR.json"
            ));
        }
        let text = crate::read_source(&e.path()).map_err(|e| format!("{rel}: {e}"))?;
        files.push((stem.to_string(), text));
    }
    if files.is_empty() {
        return Ok(None);
    }
    files.sort();
    let toml = std::fs::read_to_string(root.join("Cargo.toml")).unwrap_or_default();
    parse(&files, &settings(&toml)?).map(Some)
}

/// The locales of `files`, `(name, text)` sorted by name.
fn parse(files: &[(String, String)], set: &Settings) -> Result<Locales, String> {
    let rel = |k: usize| format!("src/locales/{}.json", files[k].0);
    // Per locale: (key, its message, line).
    let mut all: Vec<Vec<(String, Vec<Part>, u32)>> = Vec::new();
    let mut rules = Vec::new();
    for (k, (name, text)) in files.iter().enumerate() {
        let rule = plural::rule(name);
        rules.push(rule.unwrap_or(0));
        let mut msgs = Vec::new();
        for (key, value, line) in entries(text).map_err(|e| format!("{}:{e}", rel(k)))? {
            let at = |e: String| format!("{}:{line}: \"{key}\": {e}", rel(k));
            let parts = message(&value, rule, name).map_err(at)?;
            msgs.push((key, parts, line));
        }
        msgs.sort_by(|a, b| a.0.cmp(&b.0));
        if let Some(w) = msgs.windows(2).find(|w| w[0].0 == w[1].0) {
            return Err(format!(
                "{}:{}: \"{}\" is there twice",
                rel(k),
                w[1].2,
                w[1].0
            ));
        }
        all.push(msgs);
    }
    let index = |name: &str| files.iter().position(|f| f.0 == name);
    let default = match &set.default {
        None => 0,
        Some(d) => index(d)
            .ok_or_else(|| format!("Cargo.toml: i18n default {d}: no src/locales/{d}.json"))?,
    };
    let mut domains = Vec::new();
    for (host, l) in &set.domains {
        let i = index(l).ok_or_else(|| {
            format!("Cargo.toml: i18n domain {host} {l}: no src/locales/{l}.json")
        })?;
        domains.push((host.clone(), i));
    }
    // `missing warn`: what a locale lacks, it takes from the default.
    let mut warnings = Vec::new();
    if set.warn {
        let base = all[default].clone();
        for (k, msgs) in all.iter_mut().enumerate().filter(|(k, _)| *k != default) {
            let lacks =
                |m: &&(String, Vec<Part>, u32)| msgs.binary_search_by(|x| x.0.cmp(&m.0)).is_err();
            let lacking: Vec<_> = base.iter().filter(lacks).cloned().collect();
            for (key, _, line) in &lacking {
                let text = format!(
                    "\"{key}\" is missing: {} has it, and answers for this locale",
                    rel(default)
                );
                warnings.push((rel(k), *line, text));
            }
            msgs.extend(lacking);
            msgs.sort_by(|a, b| a.0.cmp(&b.0));
        }
    }
    // Every locale has every key.
    for (k, msgs) in all.iter().enumerate() {
        for (j, other) in all.iter().enumerate() {
            if let Some((key, _, line)) = other
                .iter()
                .find(|m| msgs.binary_search_by(|x| x.0.cmp(&m.0)).is_err())
            {
                return Err(format!(
                    "{}: no \"{key}\", which {}:{line} has: every locale needs every key",
                    rel(k),
                    rel(j)
                ));
            }
        }
    }
    let mut keys = Vec::new();
    for (i, (name, first, line)) in all[0].iter().enumerate() {
        let mut args: Vec<(String, bool)> = Vec::new();
        names(first, &mut args);
        for (k, locale) in all.iter().enumerate() {
            let (_, parts, l) = &locale[i];
            let mut own = Vec::new();
            names(parts, &mut own);
            let same =
                own.len() == args.len() && own.iter().all(|(n, _)| args.iter().any(|a| a.0 == *n));
            if !same {
                let list = |a: &[(String, bool)]| {
                    let v: Vec<String> = a.iter().map(|(n, _)| format!("{{{n}}}")).collect();
                    if v.is_empty() {
                        "no placeholders".into()
                    } else {
                        v.join(" ")
                    }
                };
                return Err(format!(
                    "{}:{l}: \"{name}\" has {}, but {}:{line} has {}: a translation keeps the placeholders",
                    rel(k),
                    list(&own),
                    rel(0),
                    list(&args)
                ));
            }
            for (n, counts) in own {
                if counts && let Some(a) = args.iter_mut().find(|a| a.0 == n) {
                    a.1 = true;
                }
            }
        }
        keys.push(Key {
            name: name.clone(),
            args,
            msgs: Vec::new(),
        });
    }
    for locale in all {
        for (key, (_, parts, _)) in keys.iter_mut().zip(locale) {
            key.msgs.push(parts);
        }
    }
    Ok(Locales {
        names: files.iter().map(|(n, _)| n.clone()).collect(),
        rules,
        keys,
        default,
        prefix: set.prefix,
        domains,
        warnings,
    })
}

/// The placeholders `parts` names, each once, in order, with whether a
/// plural counts by it.
fn names(parts: &[Part], out: &mut Vec<(String, bool)>) {
    for p in parts {
        let (n, counts, cases) = match p {
            Part::Text(_) => continue,
            Part::Arg(n) => (n, false, None),
            Part::Plural(n, cases) => (n, true, Some(cases)),
        };
        match out.iter_mut().find(|a| a.0 == *n) {
            Some(a) => a.1 |= counts,
            None => out.push((n.clone(), counts)),
        }
        for (_, body) in cases.into_iter().flatten() {
            names(body, out);
        }
    }
}

/// A locale file's messages: `(key, text, line)`, nested objects' keys
/// joined by `.`. The error is `line: what`.
fn entries(text: &str) -> Result<Vec<(String, String, u32)>, String> {
    let b = text.as_bytes();
    let line = |i: usize| text[..i.min(text.len())].matches('\n').count() as u32 + 1;
    let ws = |mut i: usize| {
        while i < b.len() && b[i].is_ascii_whitespace() {
            i += 1;
        }
        i
    };
    let mut out = Vec::new();
    let mut i = ws(0);
    if b.get(i) != Some(&b'{') {
        return Err(format!(
            "{}: a locale file is a JSON object of messages",
            line(i)
        ));
    }
    // The keys of the objects open around `i`.
    let mut path: Vec<String> = Vec::new();
    i += 1;
    loop {
        i = ws(i);
        match b.get(i) {
            Some(b'}') => {
                i = ws(i + 1);
                if path.pop().is_none() {
                    return match i == b.len() {
                        true => Ok(out),
                        false => Err(format!("{}: text after the object", line(i))),
                    };
                }
                if b.get(i) == Some(&b',') {
                    i += 1;
                }
                continue;
            }
            Some(b'"') => {}
            _ => return Err(format!("{}: expected a \"key\" or `}}`", line(i))),
        }
        let at = i;
        let (key, end) = string(text, i).map_err(|e| format!("{}: {e}", line(at)))?;
        i = ws(end);
        if b.get(i) != Some(&b':') {
            return Err(format!("{}: expected `:` after \"{key}\"", line(i)));
        }
        i = ws(i + 1);
        let full = path
            .iter()
            .chain([&key])
            .cloned()
            .collect::<Vec<_>>()
            .join(".");
        match b.get(i) {
            Some(b'"') => {
                let (value, end) = string(text, i).map_err(|e| format!("{}: {e}", line(i)))?;
                out.push((full, value, line(at)));
                i = ws(end);
                match b.get(i) {
                    Some(b',') => i += 1,
                    Some(b'}') => {}
                    _ => return Err(format!("{}: expected `,` or `}}`", line(i))),
                }
            }
            Some(b'{') => {
                path.push(key);
                i += 1;
            }
            _ => {
                return Err(format!(
                    "{}: \"{full}\": a message is a string, or an object of them",
                    line(i)
                ));
            }
        }
    }
}

/// The JSON string at `i` (its `"`), and the index after it.
fn string(text: &str, i: usize) -> Result<(String, usize), String> {
    let mut out = String::new();
    let mut chars = text[i + 1..].char_indices();
    while let Some((k, c)) = chars.next() {
        match c {
            '"' => return Ok((out, i + 1 + k + 1)),
            '\\' => match chars.next().map(|(_, c)| c) {
                Some('n') => out.push('\n'),
                Some('t') => out.push('\t'),
                Some('r') => out.push('\r'),
                Some('b') => out.push('\u{8}'),
                Some('f') => out.push('\u{c}'),
                Some('u') => {
                    let hex = |chars: &mut std::str::CharIndices| {
                        let h: String = chars.by_ref().take(4).map(|(_, c)| c).collect();
                        u32::from_str_radix(&h, 16).ok().filter(|_| h.len() == 4)
                    };
                    let mut u = hex(&mut chars).ok_or("a bad \\u escape")?;
                    if (0xd800..0xdc00).contains(&u) {
                        let rest = chars.as_str();
                        if rest.starts_with("\\u") {
                            chars.next();
                            chars.next();
                            let low = hex(&mut chars)
                                .filter(|l| (0xdc00..0xe000).contains(l))
                                .ok_or("a bad \\u escape")?;
                            u = 0x10000 + ((u - 0xd800) << 10) + (low - 0xdc00);
                        }
                    }
                    out.push(char::from_u32(u).ok_or("a bad \\u escape")?);
                }
                Some(c @ ('"' | '\\' | '/')) => out.push(c),
                _ => return Err("a bad escape".into()),
            },
            c if c < ' ' => return Err("a line break in a string: write \\n".into()),
            c => out.push(c),
        }
    }
    Err("a string that does not end".into())
}

/// The parts of message `s` of locale `locale` (whose plural rule is
/// `rule`, if Wisp has it).
fn message(s: &str, rule: Option<u8>, locale: &str) -> Result<Vec<Part>, String> {
    let mut p = Msg {
        s,
        i: 0,
        rule,
        locale,
    };
    let parts = p.parts(None)?;
    match p.i < s.len() {
        true => Err("a `}` with no `{`: write '}' for the brace itself".into()),
        false => Ok(parts),
    }
}

struct Msg<'a> {
    s: &'a str,
    i: usize,
    rule: Option<u8>,
    locale: &'a str,
}

impl Msg<'_> {
    fn peek(&self) -> Option<u8> {
        self.s.as_bytes().get(self.i).copied()
    }

    fn space(&mut self) {
        while self.peek().is_some_and(|c| c.is_ascii_whitespace()) {
            self.i += 1;
        }
    }

    fn word(&mut self) -> &str {
        let start = self.i;
        while self.peek().is_some_and(is_word) {
            self.i += 1;
        }
        &self.s[start..self.i]
    }

    /// Text and placeholders up to a `}` (left for the caller) or the end.
    /// `count` is the plural's argument when inside one of its cases.
    fn parts(&mut self, count: Option<&str>) -> Result<Vec<Part>, String> {
        let mut out: Vec<Part> = Vec::new();
        let mut text = String::new();
        while let Some(c) = self.peek() {
            match c {
                b'}' => break,
                b'{' => {
                    if !text.is_empty() {
                        out.push(Part::Text(std::mem::take(&mut text)));
                    }
                    self.i += 1;
                    out.push(self.placeholder()?);
                }
                b'#' if count.is_some() => {
                    if !text.is_empty() {
                        out.push(Part::Text(std::mem::take(&mut text)));
                    }
                    self.i += 1;
                    out.push(Part::Arg(count.unwrap_or_default().to_string()));
                }
                b'\'' => {
                    let next = self.s.as_bytes().get(self.i + 1).copied();
                    if next == Some(b'\'') {
                        text.push('\'');
                        self.i += 2;
                    } else if matches!(next, Some(b'{' | b'}'))
                        || (next == Some(b'#') && count.is_some())
                    {
                        let end = self.s[self.i + 1..].find('\'').ok_or(
                            "a quote ' with no end: write '' for an apostrophe before a brace",
                        )?;
                        text.push_str(&self.s[self.i + 1..self.i + 1 + end]);
                        self.i += end + 2;
                    } else {
                        text.push('\'');
                        self.i += 1;
                    }
                }
                _ => {
                    let ch = self.s[self.i..].chars().next().unwrap_or_default();
                    text.push(ch);
                    self.i += ch.len_utf8();
                }
            }
        }
        if !text.is_empty() {
            out.push(Part::Text(text));
        }
        Ok(out)
    }

    /// After a `{`: `name}` or `name, plural, cases}`.
    fn placeholder(&mut self) -> Result<Part, String> {
        self.space();
        let name = self.word().to_string();
        if name.is_empty() || name.as_bytes()[0].is_ascii_digit() {
            return Err("a placeholder is a name: {count}".into());
        }
        self.space();
        if self.peek() == Some(b'}') {
            self.i += 1;
            return Ok(Part::Arg(name));
        }
        if self.peek() != Some(b',') {
            return Err(format!("expected `}}` after {{{name}"));
        }
        self.i += 1;
        self.space();
        let kind = self.word().to_string();
        if kind != "plural" {
            return Err(format!(
                "{{{name}, {kind}, …}}: only {{{name}}} and {{{name}, plural, …}} are supported"
            ));
        }
        self.space();
        if self.peek() != Some(b',') {
            return Err(format!("expected `,` after {{{name}, plural"));
        }
        self.i += 1;
        let Some(rule) = self.rule else {
            return Err(format!(
                "Wisp has no plural rules for `{}`; name the file for a language it knows (en, fr, de, es, ru, ar, …)",
                self.locale
            ));
        };
        let mut cases: Vec<(Case, Vec<Part>)> = Vec::new();
        loop {
            self.space();
            match self.peek() {
                Some(b'}') => break,
                None => return Err(format!("{{{name}, plural, …}} does not end")),
                _ => {}
            }
            let case = if self.peek() == Some(b'=') {
                self.i += 1;
                let n = self.word();
                Case::Is(
                    n.parse()
                        .map_err(|_| format!("`={n}`: a case `=N` is a whole number"))?,
                )
            } else {
                let w = self.word().to_string();
                let cats = plural::categories(rule);
                match cats.iter().find(|&&c| plural::CATEGORIES[c as usize] == w) {
                    Some(&c) => Case::Cat(c),
                    None => {
                        let has: Vec<&str> = cats
                            .iter()
                            .map(|&c| plural::CATEGORIES[c as usize])
                            .collect();
                        return Err(format!(
                            "`{w}` is not a plural case in {}: its cases are {}, and `=N` for a number",
                            self.locale,
                            has.join(", ")
                        ));
                    }
                }
            };
            if cases.iter().any(|(c, _)| *c == case) {
                return Err("a plural case is there twice".into());
            }
            self.space();
            if self.peek() != Some(b'{') {
                return Err("a plural case's text goes in braces: one {# item}".into());
            }
            self.i += 1;
            let body = self.parts(Some(&name))?;
            if self.peek() != Some(b'}') {
                return Err("a plural case's `{` does not end".into());
            }
            self.i += 1;
            cases.push((case, body));
        }
        self.i += 1;
        if !cases.iter().any(|(c, _)| *c == Case::Cat(5)) {
            return Err(format!("{{{name}, plural, …}} needs an `other` case"));
        }
        Ok(Part::Plural(name, cases))
    }
}

/// The shell's start with `lang="{lang}"` on its `<html>`, if it has one
/// without.
pub fn with_lang(shell: &str, lang: &str) -> String {
    let Some(at) = shell.find("<html") else {
        return shell.to_string();
    };
    let end = shell[at..].find('>').map_or(shell.len(), |e| at + e);
    match shell[at..end].contains(" lang=\"") {
        true => shell.to_string(),
        false => format!("{} lang=\"{lang}\"{}", &shell[..at + 5], &shell[at + 5..]),
    }
}

impl Locales {
    /// `const HTML_LANGS`: what goes in the shell's `<html lang="…">` per
    /// locale. A right-to-left one carries its `dir` too, unless the shell
    /// has one (`ar" dir="rtl` closes the quote the shell opened).
    /// `None` with no such locale (`App::HTML_LANGS` is `LOCALES` then).
    pub fn html_langs(&self, shell: &str) -> Option<String> {
        let tag = shell.find("<html").map_or("", |at| &shell[at..]);
        let tag = &tag[..tag.find('>').unwrap_or(tag.len())];
        let rtl = |n: &String| wisp_shared::dir::is_rtl(n) && !tag.contains(" dir=");
        if !self.names.iter().any(rtl) {
            return None;
        }
        let langs: Vec<String> = (self.names.iter())
            .map(|n| match rtl(n) {
                true => lit(&format!("{n}\" dir=\"rtl")),
                false => lit(n),
            })
            .collect();
        Some(format!(
            "const HTML_LANGS: &'static [&'static str] = &[{}];",
            langs.join(", ")
        ))
    }

    /// The call `init` makes first: the default locale, prefix and domains
    /// of `i18n = [...]` (see the top of this file).
    pub fn config(&self) -> String {
        let domains: Vec<String> = (self.domains.iter())
            .map(|(h, i)| format!("({}, {i})", lit(h)))
            .collect();
        format!(
            "::wisp::rt::locale_setup({}, {}, &[{}]);",
            self.default,
            self.prefix,
            domains.join(", ")
        )
    }

    /// How many keys there are.
    pub fn key_count(&self) -> usize {
        self.keys.len()
    }

    /// The index of key `name`, by which the tables have it.
    pub fn key(&self, name: &str) -> Option<usize> {
        self.keys
            .binary_search_by(|k| k.name.as_str().cmp(name))
            .ok()
    }

    /// The placeholders of key `name`, in order, for browser code.
    pub fn args(&self, name: &str) -> Option<Vec<String>> {
        let k = self.key(name)?;
        Some(self.keys[k].args.iter().map(|(n, _)| n.clone()).collect())
    }

    /// `src`, Rust code of a template, with each `t("key", args)` as the
    /// message's table lookup (see the module's doc), marking the keys it
    /// uses in `used`.
    pub fn rust(&self, src: &str, used: &mut [bool]) -> Result<String, String> {
        let b = src.as_bytes();
        let mut out = String::with_capacity(src.len());
        let (mut i, mut at) = (0, 0);
        while i < b.len() {
            let j = skip_literal(b, i);
            if j != i {
                i = j + 1;
                continue;
            }
            let start = i;
            if !is_word(b[i]) {
                i += 1;
                continue;
            }
            while i < b.len() && is_word(b[i]) {
                i += 1;
            }
            let after_path = start > 0 && (b[start - 1] == b'.' || b[..start].ends_with(b"::"));
            let open = skip_space(b, i);
            if &b[start..i] != b"t" || after_path || b.get(open) != Some(&b'(') {
                continue;
            }
            let (args, close) = split_args(src, open)?;
            let mut args = args.into_iter();
            let key = args.next().unwrap_or_default();
            let Some(name) = key
                .strip_prefix('"')
                .and_then(|k| k.strip_suffix('"'))
                .filter(|k| !k.contains(['\\', '"']))
            else {
                return Err("t(…)'s first argument is the key, a string: t(\"cart.title\")".into());
            };
            let Some(k) = self.key(name) else {
                return Err(self.unknown(name));
            };
            used[k] = true;
            let key = &self.keys[k];
            let mut given: Vec<Option<String>> = vec![None; key.args.len()];
            let mut loose = Vec::new();
            for a in args {
                let named = named_arg(&a).or_else(|| {
                    key.args
                        .iter()
                        .any(|(n, _)| n == &a)
                        .then(|| (a.clone(), a.clone()))
                });
                match named {
                    Some((n, e)) => {
                        let Some(p) = key.args.iter().position(|(x, _)| *x == n) else {
                            return Err(format!("\"{name}\" has no {{{n}}}{}", self.has(k)));
                        };
                        if given[p].is_some() {
                            return Err(format!("{{{n}}} is given twice"));
                        }
                        given[p] = Some(self.rust(&e, used)?);
                    }
                    None => loose.push(self.rust(&a, used)?),
                }
            }
            if !loose.is_empty() {
                if loose.len() > 1 || key.args.len() != 1 || given[0].is_some() {
                    return Err(format!(
                        "name the values of \"{name}\": t(\"{name}\", count = n){}",
                        self.has(k)
                    ));
                }
                given[0] = loose.pop();
            }
            if let Some(p) = given.iter().position(Option::is_none) {
                return Err(format!(
                    "\"{name}\" needs {{{}}}{}",
                    key.args[p].0,
                    self.has(k)
                ));
            }
            out.push_str(&src[at..start]);
            if key.args.is_empty() {
                let _ = write!(out, "__wisp_i18n::K{k}[__wisp_l as usize]");
            } else {
                let values: Vec<String> = (key.args.iter().zip(given))
                    .map(|((_, counts), e)| {
                        let e = e.unwrap_or_default();
                        match counts {
                            true => {
                                format!("::wisp::rt::Arg::Num(::wisp::rt::Count::count(&({e})))")
                            }
                            false => format!("::wisp::rt::Arg::Text(&({e}))"),
                        }
                    })
                    .collect();
                let _ = write!(
                    out,
                    "::wisp::rt::Tr::new(&__wisp_i18n::K{k}[__wisp_l as usize], [{}])",
                    values.join(", ")
                );
            }
            at = close + 1;
            i = at;
        }
        out.push_str(&src[at..]);
        Ok(out)
    }

    /// `: it has {a} {b}`, or nothing, for key `k`'s errors.
    fn has(&self, k: usize) -> String {
        let a: Vec<String> = self.keys[k]
            .args
            .iter()
            .map(|(n, _)| format!("{{{n}}}"))
            .collect();
        match a.is_empty() {
            true => ": it has no placeholders".into(),
            false => format!(": it has {}", a.join(" ")),
        }
    }

    /// The error for a key no locale has.
    pub fn unknown(&self, name: &str) -> String {
        format!(
            "no \"{name}\" in src/locales/{}.json: add it to every locale",
            self.names[0]
        )
    }

    /// The tables `t(…)` reads: per key `used` marks (`K`), its message in
    /// each locale, and per key `sent` marks (`J`), what a page sends its
    /// scripts: `"key":message` as JSON, in each locale.
    pub fn tables(&self, used: &[bool], sent: &[bool]) -> String {
        let n = self.names.len();
        let mut s = String::from(
            "#[doc(hidden)]\n#[allow(dead_code, clippy::all)]\npub mod __i18n {\n    use ::wisp::rt::{Case, Msg, Part};\n",
        );
        for (k, key) in self.keys.iter().enumerate() {
            if used[k] && key.args.is_empty() {
                let texts: Vec<String> = key.msgs.iter().map(|m| lit(&plain(m))).collect();
                let _ = writeln!(
                    s,
                    "    pub static K{k}: [&str; {n}] = [{}]; // {}",
                    texts.join(", "),
                    key.name
                );
            } else if used[k] {
                let msgs: Vec<String> = (key.msgs.iter().zip(&self.rules))
                    .map(|(m, r)| {
                        format!(
                            "Msg {{ rule: {r}, parts: &[{}] }}",
                            rust_parts(m, &key.args)
                        )
                    })
                    .collect();
                let _ = writeln!(
                    s,
                    "    pub static K{k}: [Msg; {n}] = [{}]; // {}",
                    msgs.join(", "),
                    key.name
                );
            }
            if sent[k] {
                let json: Vec<String> = key
                    .msgs
                    .iter()
                    .map(|m| {
                        lit(&format!(
                            "{}:{}",
                            script_safe(&json_str(&key.name)),
                            js_message(m)
                        ))
                    })
                    .collect();
                let _ = writeln!(
                    s,
                    "    pub static J{k}: [&str; {n}] = [{}];",
                    json.join(", ")
                );
            }
        }
        s.push_str("}\n");
        s
    }
}

/// The args of the call whose `(` is at `open`, trimmed, and its `)`.
fn split_args(src: &str, open: usize) -> Result<(Vec<String>, usize), String> {
    let b = src.as_bytes();
    let (mut depth, mut start, mut i) = (0u32, open + 1, open);
    let mut out = Vec::new();
    while i < b.len() {
        let j = skip_literal(b, i);
        if j != i {
            i = j + 1;
            continue;
        }
        match b[i] {
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => {
                depth -= 1;
                if depth == 0 {
                    let last = src[start..i].trim();
                    if !last.is_empty() {
                        out.push(last.to_string());
                    }
                    return Ok((out, i));
                }
            }
            b',' if depth == 1 => {
                out.push(src[start..i].trim().to_string());
                start = i + 1;
            }
            _ => {}
        }
        i += 1;
    }
    Err("t( with no `)`".into())
}

/// `name = expr` → `(name, expr)`.
fn named_arg(a: &str) -> Option<(String, String)> {
    let (n, e) = a.split_once('=')?;
    let n = n.trim();
    let ok = !n.is_empty() && n.bytes().all(is_word) && !e.starts_with(['=', '>']);
    ok.then(|| (n.to_string(), e.trim().to_string()))
}

/// A message without placeholders, as its text.
fn plain(m: &[Part]) -> String {
    m.iter()
        .map(|p| match p {
            Part::Text(t) => t.as_str(),
            _ => "",
        })
        .collect()
}

/// `parts` as `wisp::rt::Part`s, placeholders by their index in `args`.
fn rust_parts(parts: &[Part], args: &[(String, bool)]) -> String {
    let at = |n: &str| args.iter().position(|a| a.0 == n).unwrap_or_default();
    let v: Vec<String> = parts
        .iter()
        .map(|p| match p {
            Part::Text(t) => format!("Part::Text({})", lit(t)),
            Part::Arg(n) => format!("Part::Arg({})", at(n)),
            Part::Plural(n, cases) => {
                let cases: Vec<String> = cases
                    .iter()
                    .map(|(c, body)| {
                        let c = match c {
                            Case::Is(n) => format!("Case::Is({n})"),
                            Case::Cat(c) => format!("Case::Cat({c})"),
                        };
                        format!("({c}, &[{}])", rust_parts(body, args))
                    })
                    .collect();
                format!("Part::Plural({}, &[{}])", at(n), cases.join(", "))
            }
        })
        .collect();
    v.join(", ")
}

/// A message as browser code reads it: its text alone, or an array of
/// text, `["name"]` for a placeholder and `["n", {"one": […], …}]` for a
/// plural. Safe in a `<script>`.
fn js_message(m: &[Part]) -> String {
    if m.iter().all(|p| matches!(p, Part::Text(_))) {
        return script_safe(&json_str(&plain(m)));
    }
    let v: Vec<String> = m
        .iter()
        .map(|p| match p {
            Part::Text(t) => script_safe(&json_str(t)),
            Part::Arg(n) => format!("[{}]", json_str(n)),
            Part::Plural(n, cases) => {
                let cases: Vec<String> = cases
                    .iter()
                    .map(|(c, body)| {
                        let c = match c {
                            Case::Is(n) => format!("={n}"),
                            Case::Cat(c) => plural::CATEGORIES[*c as usize].to_string(),
                        };
                        let body = match js_message(body) {
                            b if b.starts_with('[') => b,
                            b => format!("[{b}]"),
                        };
                        format!("{}:{body}", json_str(&c))
                    })
                    .collect();
                format!("[{},{{{}}}]", json_str(n), cases.join(","))
            }
        })
        .collect();
    format!("[{}]", v.join(","))
}

/// JSON with `<`, `>` and `&` escaped, so it can go in a `<script>`.
fn script_safe(json: &str) -> String {
    json.replace('<', "\\u003c")
        .replace('>', "\\u003e")
        .replace('&', "\\u0026")
}

/// A Rust string literal.
fn lit(s: &str) -> String {
    format!("{s:?}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn locales(files: &[(&str, &str)]) -> Result<Locales, String> {
        let files: Vec<(String, String)> = files
            .iter()
            .map(|(n, t)| (n.to_string(), t.to_string()))
            .collect();
        parse(&files, &Settings::default())
    }

    const EN: &str = "{\n  \"hi\": \"Hello, {name}!\",\n  \"cart\": {\n    \"items\": \"{count, plural, =0 {No items} one {# item} other {# items}}\",\n    \"title\": \"Cart\"\n  }\n}\n";
    const FR: &str = "{\"hi\": \"Bonjour {name} !\", \"cart.items\": \"{count, plural, one {# article} many {# d'articles} other {# articles}}\", \"cart\": {\"title\": \"Panier\"}}";

    #[test]
    fn surrogate_pairs() {
        assert_eq!(string(r#""😀""#, 0).unwrap().0, "\u{1f600}");
        // A high surrogate needs a low one after it, not any `\u`.
        assert!(string(r#""\ud83dA""#, 0).is_err());
        assert!(string(r#""\udc00""#, 0).is_err());
    }

    #[test]
    fn reads_and_checks_locales() {
        let l = locales(&[("en", EN), ("fr", FR)]).unwrap();
        assert_eq!(l.names, ["en", "fr"]);
        let names: Vec<&str> = l.keys.iter().map(|k| k.name.as_str()).collect();
        assert_eq!(names, ["cart.items", "cart.title", "hi"]);
        assert_eq!(l.keys[0].args, [("count".to_string(), true)]);
        assert_eq!(
            js_message(&l.keys[0].msgs[0]),
            r#"[["count",{"=0":["No items"],"one":[["count"]," item"],"other":[["count"]," items"]}]]"#
        );
        assert_eq!(js_message(&l.keys[1].msgs[1]), "\"Panier\"");
        // A key one locale lacks, or a placeholder that differs.
        let err = locales(&[
            ("en", EN),
            (
                "fr",
                "{\"hi\": \"Salut {name}\", \"cart.items\": \"{count}\"}",
            ),
        ])
        .err()
        .unwrap();
        assert_eq!(
            err,
            "src/locales/fr.json: no \"cart.title\", which src/locales/en.json:5 has: every locale needs every key"
        );
        let err = locales(&[("en", EN), ("fr", &FR.replace("{name}", "{nom}"))])
            .err()
            .unwrap();
        assert!(
            err.starts_with(
                "src/locales/fr.json:1: \"hi\" has {nom}, but src/locales/en.json:2 has {name}"
            ),
            "{err}"
        );
        // A case the language does not have, a plural with no `other`.
        let err = locales(&[("en", "{\n\"a\": \"{n, plural, few {x} other {y}}\"}")])
            .err()
            .unwrap();
        assert!(
            err.starts_with("src/locales/en.json:2: \"a\": `few` is not a plural case in en"),
            "{err}"
        );
        assert!(
            locales(&[("en", "{\"a\": \"{n, plural, one {x}}\"}")])
                .err()
                .unwrap()
                .contains("needs an `other` case")
        );
        assert!(
            locales(&[("xx", "{\"a\": \"{n, plural, other {x}}\"}")])
                .err()
                .unwrap()
                .contains("no plural rules for `xx`")
        );
        assert!(
            locales(&[("en", "{\"a\": 1}")])
                .err()
                .unwrap()
                .contains("a message is a string")
        );
        assert!(
            locales(&[("en", "{\"a\": \"x\", \"a\": \"y\"}")])
                .err()
                .unwrap()
                .contains("there twice")
        );
    }

    #[test]
    fn settings_and_fallback() {
        let toml = "[package]\nname = \"a\"\n[package.metadata.wisp]\ni18n = [\n \"default fr\", \"prefix as-needed\",\n \"domain Example.FR fr\", \"missing warn\",\n]\n";
        let set = settings(toml).unwrap();
        assert_eq!(set.default.as_deref(), Some("fr"));
        assert!(set.warn && set.prefix == 2);
        // A locale without a key takes the default's, and a warning says so.
        let files = [
            ("ar", "{\"hi\": \"Marhaba {name}\"}"),
            ("fr", FR),
            ("en", EN),
        ];
        let mut files: Vec<(String, String)> = (files.iter())
            .map(|(n, t)| (n.to_string(), t.to_string()))
            .collect();
        files.sort();
        let l = parse(&files, &set).unwrap();
        assert_eq!((l.default, l.prefix), (2, 2));
        assert_eq!(l.domains, [("example.fr".to_string(), 2)]);
        assert_eq!(l.warnings.len(), 2);
        assert_eq!(l.warnings[0].0, "src/locales/ar.json");
        assert!(l.warnings[0].2.contains("src/locales/fr.json has it"));
        let items = l.key("cart.items").unwrap();
        assert_eq!(l.keys[items].msgs[0], l.keys[items].msgs[2]);
        // Without `missing warn` it is the build error it was.
        let strict = settings("[package.metadata.wisp]\ni18n = [\"prefix always\"]\n").unwrap();
        assert!(
            parse(&files, &strict)
                .err()
                .unwrap()
                .contains("every locale needs every key")
        );
        // Mistakes are said where they are made.
        for (bad, says) in [
            ("\"prefix never\"", "optional, always or as-needed"),
            ("\"domain a b c\"", "write `default fr`"),
            ("\"domain a/b fr\"", "host name"),
            ("\"missing maybe\"", "warn or error"),
        ] {
            let toml = format!("[package.metadata.wisp]\ni18n = [{bad}]\n");
            assert!(settings(&toml).err().unwrap().contains(says), "{bad}");
        }
        let lost = Settings {
            default: Some("de".into()),
            ..Settings::default()
        };
        assert!(
            parse(&files, &lost)
                .err()
                .unwrap()
                .contains("no src/locales/de.json")
        );
        // `<html dir>`: a right-to-left locale carries it, unless the shell does.
        let shell = "<html lang=\"en\">";
        let langs = l.html_langs(shell).unwrap();
        assert!(langs.contains("\"ar\\\" dir=\\\"rtl\""), "{langs}");
        assert!(l.html_langs("<html lang=\"en\" dir=\"auto\">").is_none());
        assert!(
            locales(&[("en", EN), ("fr", FR)])
                .unwrap()
                .html_langs(shell)
                .is_none()
        );
    }

    #[test]
    fn quotes_and_escapes() {
        let m = message("It''s '{literal}' l'été {n}", Some(1), "en").unwrap();
        assert_eq!(
            m,
            [
                Part::Text("It's {literal} l'été ".into()),
                Part::Arg("n".into())
            ]
        );
        assert!(message("a } b", Some(1), "en").is_err());
        assert!(message("{1}", Some(1), "en").is_err());
        assert!(message("{n, select, a {x} other {y}}", Some(1), "en").is_err());
        let (s, end) = string("\"a\\n\\u00e9\\ud83d\\ude00\" x", 0).unwrap();
        assert_eq!((s.as_str(), end), ("a\né😀", 23));
    }

    #[test]
    fn rewrites_template_calls() {
        let l = locales(&[("en", EN), ("fr", FR)]).unwrap();
        let mut used = vec![false; 3];
        assert_eq!(
            l.rust("t(\"cart.title\")", &mut used).unwrap(),
            "__wisp_i18n::K1[__wisp_l as usize]"
        );
        assert_eq!(
            l.rust("t(\"cart.items\", cart.len())", &mut used).unwrap(),
            "::wisp::rt::Tr::new(&__wisp_i18n::K0[__wisp_l as usize], [::wisp::rt::Arg::Num(::wisp::rt::Count::count(&(cart.len())))])"
        );
        assert_eq!(
            l.rust("x.t(\"no\") + t (\"hi\", name = user.name)", &mut used)
                .unwrap(),
            "x.t(\"no\") + ::wisp::rt::Tr::new(&__wisp_i18n::K2[__wisp_l as usize], [::wisp::rt::Arg::Text(&(user.name))])"
        );
        assert_eq!(used, [true, true, true]);
        assert!(l.rust("t(\"hi\", name)", &mut used).is_ok());
        let err = |s: &str| l.rust(s, &mut [false; 3]).unwrap_err();
        assert_eq!(
            err("t(\"nope\")"),
            "no \"nope\" in src/locales/en.json: add it to every locale"
        );
        assert!(err("t(key)").contains("first argument is the key"));
        assert_eq!(err("t(\"hi\")"), "\"hi\" needs {name}: it has {name}");
        assert_eq!(
            err("t(\"hi\", who = 1)"),
            "\"hi\" has no {who}: it has {name}"
        );
        assert!(err("t(\"cart.title\", 1)").contains("name the values"));
    }
}
