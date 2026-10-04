//! `redirects`, `rewrites` and `headers` of `[package.metadata.wisp]` in
//! the app's Cargo.toml, as the next.config of Next.js has them, checked
//! here and baked into tables (the runtime half is `wisp::rt::redirect`,
//! `rewrite` and `headers`). Each is a list of strings:
//!
//! ```toml
//! redirects = ["/old/[id] /new/[id] 301", "/docs https://docs.example.com"]
//! rewrites  = ["/guide/[...p] /docs/[...p]"]
//! headers   = ["/api/[...p] access-control-allow-origin: *"]
//! ```
//!
//! A pattern is a route's: text, `[name]`, a last `[...name]`. A redirect is
//! `from to [status]` (308 unless told, 301, 302, 303 or 307); its `to` is a
//! path of the app or an `http(s)://` URL, its `[name]`s from `from`. A
//! rewrite is `from route`: `route` is a route of the app, its pattern as
//! it is (`/docs/[...p]`), no matcher or optional segment, and each of its
//! parameters a `[name]` of `from`. A header is `pattern name: value`.
//! None declared, none compiled: the server does nothing for them.

use crate::routes::{MAX_PARAMS, Seg, Tree};
use std::path::Path;

#[derive(Default)]
pub struct Rules {
    redirects: Vec<(String, String, u16)>,
    rewrites: Vec<(String, usize, Vec<String>)>,
    headers: Vec<(String, String, String)>,
}

pub fn load(root: &Path, tree: &Tree) -> Result<Rules, String> {
    let toml = std::fs::read_to_string(root.join("Cargo.toml")).unwrap_or_default();
    parse(&toml, tree)
}

fn parse(toml: &str, tree: &Tree) -> Result<Rules, String> {
    let mut rules = Rules::default();
    for e in strings(toml, "redirects")? {
        let at = |m: &str| format!("Cargo.toml: redirects = [\"{e}\"]: {m}");
        let mut w = e.split_whitespace();
        let (Some(from), Some(to)) = (w.next(), w.next()) else {
            return Err(at("write `from to` or `from to status`"));
        };
        let status = match w.next() {
            None => 308,
            Some(s) => s
                .parse::<u16>()
                .ok()
                .filter(|s| [301, 302, 303, 307, 308].contains(s))
                .ok_or_else(|| at("the status is 301, 302, 303, 307 or 308"))?,
        };
        if w.next().is_some() {
            return Err(at("write `from to` or `from to status`"));
        }
        let names = pattern(from).map_err(|m| at(&m))?;
        let host = to
            .find("://")
            .map_or(0, |i| to[i + 3..].find('/').map_or(to.len(), |j| i + 3 + j));
        match to {
            _ if to.starts_with("//") => {
                return Err(at("`to` is a path of the app or an http(s):// URL"));
            }
            _ if to.starts_with('/') => {}
            _ if to.starts_with("http://") || to.starts_with("https://") => {
                if to[..host].contains('[') {
                    return Err(at("no `[name]` in the URL's host"));
                }
            }
            _ => return Err(at("`to` is a path of the app or an http(s):// URL")),
        }
        if to.contains(|c: char| c.is_control() || c == '\\' || c == '"') {
            return Err(at("`to` has a character a URL cannot"));
        }
        for used in holes(&to[host..]) {
            if !names.contains(&used) {
                return Err(at(&format!("`[{used}]` is not in `{from}`")));
            }
        }
        rules.redirects.push((from.into(), to.into(), status));
    }
    rules.no_redirect_loop()?;
    for e in strings(toml, "rewrites")? {
        let at = |m: &str| format!("Cargo.toml: rewrites = [\"{e}\"]: {m}");
        let w: Vec<&str> = e.split_whitespace().collect();
        let [from, to] = w[..] else {
            return Err(at("write `from route`, the route as it is: `/docs/[...p]`"));
        };
        let names = pattern(from).map_err(|m| at(&m))?;
        let Some(id) = tree.routes.iter().position(|r| r.pattern() == to) else {
            return Err(at(&format!(
                "`{to}` is not a route of the app (a route's pattern, as `wisp routes` shows it)"
            )));
        };
        let route = &tree.routes[id];
        if route
            .segs
            .iter()
            .any(|s| matches!(s, Seg::Optional(..) | Seg::Param(_, Some(_))))
        {
            return Err(at(
                "the route has a matcher or an optional segment, which a rewrite does not check: route to one without",
            ));
        }
        let params: Vec<String> = route.params().iter().map(|p| p.to_string()).collect();
        for p in &params {
            if !names.iter().any(|n| n == p) {
                return Err(at(&format!(
                    "the route takes `{p}`: `{from}` has no `[{p}]`"
                )));
            }
            // One segment into a rest, or a rest into one, is not what either asked.
            let rest = format!("[...{p}]");
            if from.contains(&rest) != to.contains(&rest) {
                return Err(at(&format!(
                    "`{p}` is `[...{p}]` in only one of `{from}` and `{to}`"
                )));
            }
        }
        rules.rewrites.push((from.into(), id, params));
    }
    for e in strings(toml, "headers")? {
        let at = |m: &str| format!("Cargo.toml: headers = [\"{e}\"]: {m}");
        let Some((from, header)) = e.split_once(char::is_whitespace) else {
            return Err(at("write `pattern name: value`"));
        };
        pattern(from).map_err(|m| at(&m))?;
        let Some((name, value)) = header.split_once(':') else {
            return Err(at("write `pattern name: value`"));
        };
        let (name, value) = (name.trim().to_ascii_lowercase(), value.trim());
        let token = |c: char| c.is_ascii_alphanumeric() || "!#$%&'*+-.^_`|~".contains(c);
        if name.is_empty() || !name.chars().all(token) {
            return Err(at("the header's name is a token"));
        }
        if [
            "content-length",
            "transfer-encoding",
            "connection",
            "set-cookie",
        ]
        .contains(&name.as_str())
        {
            return Err(at(
                "Wisp frames the reply and `Cx` sets cookies: not a header to set here",
            ));
        }
        if value.chars().any(|c| c.is_control()) {
            return Err(at("the value has a control character"));
        }
        rules.headers.push((from.into(), name, value.into()));
    }
    Ok(rules)
}

/// The names of `pattern`, or what is wrong with it.
fn pattern(p: &str) -> Result<Vec<&str>, String> {
    if !p.starts_with('/') {
        return Err(format!("`{p}` starts with a `/`"));
    }
    let segs: Vec<&str> = p[1..].split('/').collect();
    let mut names: Vec<&str> = Vec::new();
    for (i, s) in segs.iter().enumerate() {
        if s.is_empty() && p.len() > 1 {
            return Err(format!("`{p}` has an empty segment: no `//`, no end `/`"));
        }
        let hole = s.strip_prefix('[').and_then(|s| s.strip_suffix(']'));
        let Some(h) = hole else {
            if s.contains(['[', ']']) {
                return Err(format!("`{p}`: `{s}` is text, `[name]` or `[...name]`"));
            }
            continue;
        };
        let (name, rest) = match h.strip_prefix("...") {
            Some(n) => (n, true),
            None => (h, false),
        };
        if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            return Err(format!(
                "`{p}`: `{s}` names a parameter with letters, digits and `_`"
            ));
        }
        if rest && i + 1 != segs.len() {
            return Err(format!("`{p}`: `[...{name}]` is the last segment"));
        }
        if names.contains(&name) {
            return Err(format!("`{p}`: `{name}` twice"));
        }
        names.push(name);
    }
    if names.len() > MAX_PARAMS {
        return Err(format!("`{p}` has more than {MAX_PARAMS} parameters"));
    }
    Ok(names)
}

/// The names of the `[name]`s and `[...name]`s in `s`.
fn holes(s: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut rest = s;
    while let Some(i) = rest.find('[') {
        let Some(j) = rest[i..].find(']') else { break };
        out.push(rest[i + 1..i + j].trim_start_matches("..."));
        rest = &rest[i + j + 1..];
    }
    out
}

/// The strings of `key = [...]` in `[package.metadata.wisp]`, or what is
/// wrong with the list. A `[` or `]` inside a string is text.
pub fn strings(toml: &str, key: &str) -> Result<Vec<String>, String> {
    let bad = |m: &str| format!("Cargo.toml: {key} = [...]: {m}");
    let (mut on, mut start, mut at) = (false, None, 0);
    for l in toml.split_inclusive('\n') {
        let t = l.split('#').next().unwrap_or("").trim();
        if t.starts_with('[') && !t.starts_with("[\"") {
            on = t == "[package.metadata.wisp]";
        } else if on
            && let Some(r) = t.strip_prefix(key).map(str::trim_start)
            && r.starts_with('=')
        {
            start = Some(at + (l.len() - l.trim_start().len()) + key.len());
            break;
        }
        at += l.len();
    }
    let Some(start) = start else {
        return Ok(Vec::new());
    };
    let mut c = toml[start..].chars().peekable();
    let skip = |c: &mut std::iter::Peekable<std::str::Chars>| {
        while let Some(&ch) = c.peek() {
            match ch {
                '#' => c.by_ref().take_while(|&x| x != '\n').for_each(drop),
                _ if ch.is_whitespace() => drop(c.next()),
                _ => break,
            }
        }
    };
    skip(&mut c);
    let eq = c.next();
    skip(&mut c);
    if (eq, c.next()) != (Some('='), Some('[')) {
        return Err(bad("write a list of strings: [\"…\", \"…\"]"));
    }
    let mut out = Vec::new();
    let mut comma = true;
    loop {
        skip(&mut c);
        match c.next() {
            None => return Err(bad("the list has no closing `]`")),
            Some(']') => return Ok(out),
            Some(',') => comma = true,
            Some(_) if !comma => return Err(bad("a comma goes between strings")),
            Some(q @ ('"' | '\'')) => {
                let mut s = String::new();
                loop {
                    match c.next() {
                        Some(x) if x == q => break,
                        Some('\\') if q == '"' => match c.next() {
                            Some(x @ ('"' | '\\')) => s.push(x),
                            Some('t') => s.push('\t'),
                            _ => {
                                return Err(bad(
                                    "a string's escapes are \\\" and \\\\: or use 'single quotes'",
                                ));
                            }
                        },
                        Some('\n') | None => return Err(bad("a string has no closing quote")),
                        Some(x) => s.push(x),
                    }
                }
                out.push(s);
                comma = false;
            }
            Some(_) => return Err(bad("the list holds strings: \"from to\"")),
        }
    }
}

impl Rules {
    /// A redirect whose `to` leads, rule by rule, back to where it began:
    /// the browser would be sent round for ever. Judged with each `[name]`
    /// of a `to` as `x`; a loop that needs other values goes unseen.
    fn no_redirect_loop(&self) -> Result<(), String> {
        for (k, (from, _, _)) in self.redirects.iter().enumerate() {
            let mut seen = vec![k];
            let mut at = k;
            while self.redirects[at].1.starts_with('/') {
                let to = self.redirects[at].1.split(['?', '#']).next().unwrap_or("");
                let to = concrete(to);
                let Some(next) = self.redirects.iter().position(|r| fits(&r.0, &to)) else {
                    break;
                };
                if seen.contains(&next) {
                    if next != k {
                        break; // a loop that does not include `k` is its own rule's to report
                    }
                    let way: Vec<&str> =
                        seen.iter().map(|&i| self.redirects[i].0.as_str()).collect();
                    return Err(format!(
                        "Cargo.toml: redirects: `{from}` goes round for ever: {} -> {from}",
                        way.join(" -> ")
                    ));
                }
                seen.push(next);
                at = next;
            }
        }
        Ok(())
    }

    /// The `impl App` items (one per line) of the rules there are.
    pub fn emit(&self) -> Vec<String> {
        let lit = |s: &str| format!("{s:?}");
        let mut out = Vec::new();
        if !self.redirects.is_empty() {
            let rows: Vec<String> = (self.redirects.iter())
                .map(|(f, t, s)| format!("({}, {}, {s})", lit(f), lit(t)))
                .collect();
            out.push("const REDIRECTS: bool = true;".into());
            out.push("fn redirect(cx: &::wisp::Cx, reply: &mut ::wisp::Reply) -> bool {".into());
            out.push(format!(
                "    const RULES: &[(&str, &str, u16)] = &[{}];",
                rows.join(", ")
            ));
            out.push("    ::wisp::rt::redirect(cx, reply, RULES)".into());
            out.push("}".into());
        }
        if !self.rewrites.is_empty() {
            let rows: Vec<String> = (self.rewrites.iter())
                .map(|(f, id, names)| {
                    let names: Vec<String> = names.iter().map(|n| lit(n)).collect();
                    format!("({}, {id}, &[{}])", lit(f), names.join(", "))
                })
                .collect();
            out.push("const REWRITES: bool = true;".into());
            out.push(
                "fn rewrite(path: &str) -> Option<(usize, [&str; ::wisp::rt::MAX_PARAMS])> {"
                    .into(),
            );
            out.push(format!(
                "    const RULES: &[(&str, usize, &[&str])] = &[{}];",
                rows.join(", ")
            ));
            out.push("    ::wisp::rt::rewrite(path, RULES)".into());
            out.push("}".into());
        }
        if !self.headers.is_empty() {
            let rows: Vec<String> = (self.headers.iter())
                .map(|(p, n, v)| format!("({}, {}, {})", lit(p), lit(n), lit(v)))
                .collect();
            out.push("const HEADERS: bool = true;".into());
            out.push("fn headers(cx: &::wisp::Cx, reply: &mut ::wisp::Reply) {".into());
            out.push(format!(
                "    const RULES: &[(&str, &str, &str)] = &[{}];",
                rows.join(", ")
            ));
            out.push("    ::wisp::rt::headers(cx, reply, RULES)".into());
            out.push("}".into());
        }
        out
    }
}

/// `path` with each `[name]` or `[...name]` as `x`.
fn concrete(path: &str) -> String {
    let (mut out, mut rest) = (String::new(), path);
    while let Some(i) = rest.find('[') {
        out += &rest[..i];
        out.push('x');
        rest = rest[i..].split_once(']').map_or("", |r| r.1);
    }
    out + rest
}

/// Whether `path` is one of `pattern`'s.
fn fits(pattern: &str, path: &str) -> bool {
    let p: Vec<&str> = pattern.split('/').collect();
    let r: Vec<&str> = path.split('/').collect();
    for (k, s) in p.iter().enumerate() {
        if s.starts_with("[...") {
            return r.len() + 1 >= p.len();
        }
        let ok = match r.get(k) {
            Some(x) if s.starts_with('[') => !x.is_empty(),
            Some(x) => x == s,
            None => false,
        };
        if !ok {
            return false;
        }
    }
    p.len() == r.len()
}
