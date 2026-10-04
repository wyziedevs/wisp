//! The rules of `[package.metadata.wisp]` (`redirects`, `rewrites`,
//! `headers`): the build checks them and bakes the tables into the app (see
//! `wisp-build`'s `rules.rs`), these walk them. An app without any never
//! calls in: `App::REDIRECTS`, `REWRITES` and `HEADERS` are consts.
//!
//! A pattern is a path whose segments are text, `[name]` (one segment) or
//! `[...name]` (the rest, none included), as a route's.

use crate::cx::MAX_PARAMS;
use crate::{Cx, Reply};
use std::borrow::Cow;

type Got<'a> = [(&'static str, &'a str); MAX_PARAMS];

/// Matches `path` to `pattern`, its captures into `got`: how many.
fn capture<'a>(pattern: &'static str, path: &'a str, got: &mut Got<'a>) -> Option<usize> {
    let seg = crate::rt::seg;
    let (mut p, mut r) = (pattern.strip_prefix('/')?, path.strip_prefix('/')?);
    let mut n = 0;
    loop {
        let (ps, pn) = seg(p);
        if let Some(name) = ps.strip_prefix("[...").and_then(|s| s.strip_suffix(']')) {
            *got.get_mut(n)? = (name, r);
            return Some(n + 1);
        }
        let (rs, rn) = seg(r);
        match ps.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
            Some(name) if !rs.is_empty() => {
                *got.get_mut(n)? = (name, rs);
                n += 1;
            }
            Some(_) => return None,
            None if ps != rs => return None,
            None => {}
        }
        match (pn, rn) {
            (None, None) => return Some(n),
            (Some(a), Some(b)) => (p, r) = (a, b),
            // `/a/[...rest]` takes `/a` too: no rest.
            (Some(a), None) => {
                let name = a.strip_prefix("[...").and_then(|s| s.strip_suffix(']'))?;
                *got.get_mut(n)? = (name, "");
                return Some(n + 1);
            }
            (None, Some(_)) => return None,
        }
    }
}

/// A 308 (or the rule's status) to the first rule of `rules` whose `from`
/// matches the request's path: `to` with its `[name]`s filled in, under the
/// base path when it is a path of the app, the query kept. Whether one did.
pub fn redirect(cx: &Cx, reply: &mut Reply, rules: &[(&'static str, &'static str, u16)]) -> bool {
    let mut got = [("", ""); MAX_PARAMS];
    for &(from, to, status) in rules {
        let Some(n) = capture(from, cx.path(), &mut got) else {
            continue;
        };
        let (mut at, mut rest) = (String::new(), to);
        while let Some(i) = rest.find('[') {
            let j = rest[i..].find(']').map_or(rest.len(), |j| i + j);
            let name = rest[i + 1..j].trim_start_matches("...");
            at += &rest[..i];
            at += got[..n].iter().find(|g| g.0 == name).map_or("", |g| g.1);
            rest = rest.get(j + 1..).unwrap_or("");
        }
        at += rest;
        let mut location = match at.starts_with('/') {
            // `//evil.example` would be another site's.
            true => format!(
                "{}/{}",
                crate::protocol::BASE,
                at.trim_start_matches(['/', '\\'])
            ),
            false => at,
        };
        let query = cx.query_string();
        if !query.is_empty() {
            location.push(if location.contains('?') { '&' } else { '?' });
            location += query;
        }
        reply.set_plain(status, "");
        reply
            .headers
            .push((Cow::Borrowed("location"), Cow::Owned(location)));
        return true;
    }
    false
}

/// The route of the first rule of `rules` (`from`, route, the route's
/// parameters by name) that matches `path`, with its parameters taken from
/// the `[name]`s of `from`: the request is served by that route, the URL
/// staying as it is. The build proved each name is in `from`.
pub fn rewrite<'a>(
    path: &'a str,
    rules: &[(&'static str, usize, &[&'static str])],
) -> Option<(usize, [&'a str; MAX_PARAMS])> {
    let mut got = [("", ""); MAX_PARAMS];
    for &(from, route, names) in rules {
        let Some(n) = capture(from, path, &mut got) else {
            continue;
        };
        let mut params = [""; MAX_PARAMS];
        for (k, name) in names.iter().enumerate() {
            params[k] = got[..n].iter().find(|g| g.0 == *name)?.1;
        }
        return Some((route, params));
    }
    None
}

/// Sets each header of `rules` (`pattern`, name, value) whose pattern
/// matches the request's path on `reply`, replacing the one it has.
pub fn headers(cx: &Cx, reply: &mut Reply, rules: &[(&'static str, &'static str, &'static str)]) {
    let mut got = [("", ""); MAX_PARAMS];
    for &(pattern, name, value) in rules {
        if capture(pattern, cx.path(), &mut got).is_none() {
            continue;
        }
        let value = Cow::Borrowed(value);
        match reply.headers.iter_mut().find(|h| h.0 == name) {
            Some(h) => h.1 = value,
            None => reply.headers.push((Cow::Borrowed(name), value)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn got(pattern: &'static str, path: &str) -> Option<Vec<(&'static str, String)>> {
        let mut g = [("", ""); MAX_PARAMS];
        let n = capture(pattern, path, &mut g)?;
        Some(g[..n].iter().map(|(k, v)| (*k, v.to_string())).collect())
    }

    #[test]
    fn patterns_match_like_routes() {
        let one = |k: &'static str, v: &str| Some(vec![(k, v.to_string())]);
        assert_eq!(got("/", "/"), Some(vec![]));
        assert_eq!(got("/a", "/a"), Some(vec![]));
        assert_eq!(got("/a", "/b"), None);
        assert_eq!(got("/a", "/a/b"), None);
        assert_eq!(got("/a/b", "/a"), None);
        assert_eq!(got("/a/[id]", "/a/7"), one("id", "7"));
        assert_eq!(got("/a/[id]", "/a/"), None);
        assert_eq!(got("/a/[id]/x", "/a/7/x"), one("id", "7"));
        assert_eq!(got("/a/[...p]", "/a/b/c"), one("p", "b/c"));
        assert_eq!(got("/a/[...p]", "/a"), one("p", ""));
        assert_eq!(got("/[...p]", "/"), one("p", ""));
        assert_eq!(got("/a/[...p]", "/b/c"), None);
    }
}
