//! The `content-security-policy` of pages and error pages. Made once,
//! after `init`, from the defaults, the app's changes ([`csp`]) and the
//! hashes of its inline scripts, which the build knows (no holes go in a
//! `<script>`): each answer adds one header of a fixed string, baked and
//! `CACHE` pages too, and no script runs that the app did not write.
//!
//! Dev mode adds what its tools load: npm modules from esm.sh, and the
//! reload events from `wisp dev`.

use std::sync::{Mutex, OnceLock};

/// The policy, but `script-src`'s hashes and dev's sources.
const DEFAULT: [(&str, &str); 8] = [
    ("default-src", "'self'"),
    ("script-src", "'self'"),
    ("style-src", "'self' 'unsafe-inline'"),
    ("img-src", "'self' data: https:"),
    ("connect-src", "'self'"),
    ("base-uri", "'self'"),
    ("form-action", "'self'"),
    ("frame-ancestors", "'self'"),
];

/// What `init` said: directives that replace the defaults' of their name
/// (or add one), and whether the header is off.
static CHANGES: Mutex<(String, bool)> = Mutex::new((String::new(), false));
static HEADER: OnceLock<Option<&'static str>> = OnceLock::new();

/// Changes the `content-security-policy` pages get, in `init`:
/// `wisp::csp("img-src 'self' https://cdn.example; font-src https://fonts.gstatic.com")`.
/// Each directive replaces the default one of its name, or is added.
/// `script-src` keeps the hashes of the app's inline scripts.
pub fn csp(directives: &str) {
    let mut c = CHANGES.lock().unwrap_or_else(|e| e.into_inner());
    c.0.push_str(directives);
    c.0.push(';');
}

/// No `content-security-policy` from Wisp, for an app that sets its own
/// (or none). In `init`.
pub fn csp_off() {
    CHANGES.lock().unwrap_or_else(|e| e.into_inner()).1 = true;
}

/// Makes the header, once: after `init`, which may change it. An app with
/// a service worker (`worker`) says `worker-src 'self'`, before its own
/// changes, which may say otherwise.
pub(crate) fn ready(hashes: &[&str], worker: bool) {
    HEADER.get_or_init(|| {
        let (mut changes, off) = CHANGES.lock().unwrap_or_else(|e| e.into_inner()).clone();
        if worker {
            changes.insert_str(0, "worker-src 'self';");
        }
        let dev = crate::settings()
            .dev
            .then(|| crate::dev::events_port().map(|p| format!("http://127.0.0.1:{p}")));
        (!off).then(|| &*Box::leak(policy(hashes, &changes, dev).into_boxed_str()))
    });
}

/// The header's value, `None` when it is off (or before [`ready`]).
#[inline]
pub(crate) fn header() -> Option<&'static str> {
    HEADER.get().copied().flatten()
}

/// The policy: the defaults, `changes` over them, the inline scripts'
/// `hashes` in `script-src`, and in dev (`Some`, with the reload events'
/// origin when `wisp dev` runs it) esm.sh.
fn policy(hashes: &[&str], changes: &str, dev: Option<Option<String>>) -> String {
    let mut all: Vec<(String, String)> = (DEFAULT.iter())
        .map(|(n, v)| (n.to_string(), v.to_string()))
        .collect();
    for d in changes.split(';').map(str::trim).filter(|d| !d.is_empty()) {
        let (name, sources) = d.split_once(char::is_whitespace).unwrap_or((d, ""));
        let name = name.to_ascii_lowercase();
        match all.iter_mut().find(|(n, _)| *n == name) {
            Some(had) => had.1 = sources.trim().to_string(),
            None => all.push((name, sources.trim().to_string())),
        }
    }
    let add = |all: &mut Vec<(String, String)>, name: &str, source: &str| {
        if let Some((_, v)) = all.iter_mut().find(|(n, _)| n == name) {
            v.push(' ');
            v.push_str(source);
        }
    };
    // A hash would turn an `'unsafe-inline'` off.
    let inline = all
        .iter()
        .any(|(n, v)| n == "script-src" && v.contains("'unsafe-inline'"));
    for h in hashes.iter().filter(|_| !inline) {
        add(&mut all, "script-src", h);
    }
    if let Some(events) = dev {
        add(&mut all, "script-src", "https://esm.sh");
        add(&mut all, "connect-src", "https://esm.sh");
        if let Some(e) = events {
            add(&mut all, "connect-src", &e);
        }
    }
    let parts: Vec<String> = (all.iter())
        .map(|(n, v)| {
            if v.is_empty() {
                n.clone()
            } else {
                format!("{n} {v}")
            }
        })
        .collect();
    parts.join("; ")
}

#[cfg(test)]
mod tests {
    use super::policy;

    #[test]
    fn the_default_with_hashes() {
        assert_eq!(
            policy(&["'sha256-a'", "'sha256-b'"], "", None),
            "default-src 'self'; script-src 'self' 'sha256-a' 'sha256-b'; \
             style-src 'self' 'unsafe-inline'; img-src 'self' data: https:; \
             connect-src 'self'; base-uri 'self'; form-action 'self'; frame-ancestors 'self'"
        );
    }

    #[test]
    fn changes_replace_or_add() {
        let p = policy(
            &["'sha256-a'"],
            "IMG-SRC https:; font-src https://f.example;; frame-ancestors 'none'; upgrade-insecure-requests;",
            None,
        );
        assert!(p.contains("; img-src https:;"), "{p}");
        assert!(
            p.contains(
                "; frame-ancestors 'none'; font-src https://f.example; upgrade-insecure-requests"
            ),
            "{p}"
        );
        let p = policy(
            &["'sha256-a'"],
            "script-src 'self' https://cdn.example",
            None,
        );
        assert!(
            p.contains("script-src 'self' https://cdn.example 'sha256-a';"),
            "{p}"
        );
        // `'unsafe-inline'` stays on: a hash beside it would turn it off.
        let p = policy(&["'sha256-a'"], "script-src 'self' 'unsafe-inline'", None);
        assert!(p.contains("script-src 'self' 'unsafe-inline';"), "{p}");
    }

    #[test]
    fn a_service_worker_is_allowed() {
        let p = policy(&[], "worker-src 'self';", None);
        assert!(
            p.ends_with("; frame-ancestors 'self'; worker-src 'self'"),
            "{p}"
        );
        let p = policy(&[], "worker-src 'self';worker-src 'none';", None);
        assert!(
            p.ends_with("; worker-src 'none'"),
            "the app's own wins: {p}"
        );
    }

    #[test]
    fn dev_loads_npm_and_reload_events() {
        let p = policy(&[], "", Some(Some("http://127.0.0.1:4000".into())));
        assert!(p.contains("script-src 'self' https://esm.sh;"), "{p}");
        assert!(
            p.contains("connect-src 'self' https://esm.sh http://127.0.0.1:4000;"),
            "{p}"
        );
    }
}
