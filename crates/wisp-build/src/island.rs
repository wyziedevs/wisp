//! `<Island of="react:pkg#Name" client:visible props={:{ a: 1 }} />`: a
//! React, Preact, Vue or Svelte component from npm (or `$lib`), drawn by
//! its framework in the browser.
//!
//! Rewritten before the parser reads the file, into what the parser
//! already knows: a `<div data-wisp-keep>` with the `client:` wait and a
//! `use:` that mounts the component, and at the end of the file's script
//! the imports (bare ones go through the app's npm packages) and a small
//! mount function per framework. Lines stay where they were, so errors and
//! source maps still point at the file as written. A file without
//! `<Island` is read as it is: no cost.

use crate::template::Error;

/// The frameworks, each with its imports and the mount function made of
/// them: `(el, props) => { update, destroy }`, as `use:` wants. `__props`
/// is the props: a `props={:…}` value, else the server's JSON.
const MOUNTS: [(&str, &str); 4] = [
    (
        "react",
        "import { createElement as __h_react } from 'react';\
         import { createRoot as __root_react } from 'react-dom/client';\
         const __mount_react = (C) => (el, p) => { el.textContent = ''; const r = __root_react(el); \
         const draw = (p) => r.render(__h_react(C, p)); draw(__props(el, p)); \
         return { update: draw, destroy: () => r.unmount() } };",
    ),
    (
        "preact",
        "import { h as __h_preact, render as __render_preact } from 'preact';\
         const __mount_preact = (C) => (el, p) => { el.textContent = ''; \
         const draw = (p) => __render_preact(__h_preact(C, p), el); draw(__props(el, p)); \
         return { update: draw, destroy: () => __render_preact(null, el) } };",
    ),
    (
        "vue",
        "import { createApp as __app_vue, h as __h_vue, reactive as __state_vue } from 'vue';\
         const __mount_vue = (C) => (el, p) => { const s = __state_vue({ ...__props(el, p) }); \
         const app = __app_vue({ render: () => __h_vue(C, { ...s }) }); el.textContent = ''; app.mount(el); \
         return { update: (p) => Object.assign(s, p), destroy: () => app.unmount() } };",
    ),
    (
        "svelte",
        "import { mount as __put_svelte, unmount as __drop_svelte } from 'svelte';\
         const __mount_svelte = (C) => (el, p) => { let c; \
         const draw = (p) => { if (c) __drop_svelte(c); el.textContent = ''; c = __put_svelte(C, { target: el, props: p }) }; \
         draw(__props(el, p)); return { update: draw, destroy: () => __drop_svelte(c) } };",
    ),
];

const PROPS: &str = "const __props = (el, p) => p ?? JSON.parse(el.dataset.props || '{}');";

/// `src` with each `<Island>` made plain, or `None` when it has none.
pub fn expand(src: &str) -> Result<Option<String>, Error> {
    if !src.contains("<Island") {
        return Ok(None);
    }
    let b = src.as_bytes();
    let mut out = String::with_capacity(src.len() + 512);
    let mut js = String::new();
    let mut used: Vec<&str> = Vec::new();
    // The client script's end (where its `</script>` starts), and whether
    // it is TypeScript.
    let mut script: Option<(usize, bool)> = None;
    let mut n = 0;
    let (mut at, mut i) = (0, 0);
    while let Some(k) = src[i..].find('<').map(|k| i + k) {
        let rest = &src[k..];
        if let Some(r) = rest.strip_prefix("<!--") {
            i = k + 4 + r.find("-->").map_or(r.len(), |e| e + 3);
            continue;
        }
        let named = |t: &str| {
            rest.as_bytes()
                .get(1..=t.len())
                .is_some_and(|n| n.eq_ignore_ascii_case(t.as_bytes()))
                && !rest
                    .as_bytes()
                    .get(t.len() + 1)
                    .is_some_and(u8::is_ascii_alphanumeric)
        };
        if let Some(tag) = ["script", "style"].into_iter().find(|t| named(t)) {
            let gt = rest.find('>').map_or(src.len(), |e| k + e);
            let attrs = src[k + 1 + tag.len()..gt].trim();
            let open = (gt + 1).min(src.len());
            let close = src[open..]
                .find(&format!("</{tag}"))
                .map_or(src.len(), |e| open + e);
            if tag == "script" && script.is_none() {
                let ts = matches!(attrs, "lang=\"ts\"" | "lang='ts'");
                if attrs.is_empty() || ts {
                    script = Some((close, ts));
                }
            }
            i = close;
            continue;
        }
        let is_island = rest.strip_prefix("<Island").is_some_and(|r| {
            r.starts_with(|c: char| c.is_ascii_whitespace() || c == '/' || c == '>')
        });
        if !is_island {
            i = k + 1;
            continue;
        }
        let err = |at: usize, msg: String| {
            let line = src[..at].matches('\n').count() as u32 + 1;
            let col = (at - src[..at].rfind('\n').map_or(0, |n| n + 1)) as u32 + 1;
            Error { line, col, msg }
        };
        let tag = Tag::read(b, k + "<Island".len()).map_err(|(at, m)| err(at, m))?;
        let Some(of) = tag.of else {
            return Err(err(k, "<Island> needs of=\"framework:module\", like of=\"react:react-switch\" or of=\"vue:$lib/Chart.js#Chart\"".into()));
        };
        let (framework, module, name) = spec(&of).map_err(|m| err(k, m))?;
        if !used.contains(&framework) {
            used.push(framework);
        }
        let comp = format!("__island{n}");
        js.push_str(&match name {
            Some(name) => format!("import {{ {name} as {comp} }} from '{module}';"),
            None => format!("import {comp} from '{module}';"),
        });
        js.push_str(&format!(
            "const {comp}_mount = __mount_{framework}({comp});"
        ));
        // The element, on the tag's lines: line breaks kept.
        out.push_str(&src[at..k]);
        out.push_str("<div data-wisp-keep");
        out.push_str(&tag.attrs);
        match &tag.props {
            Props::None => out.push_str(&format!(" use:{comp}_mount")),
            Props::Server(rust) => out.push_str(&format!(
                " data-props={{::wisp::to_json(&({rust}))}} use:{comp}_mount"
            )),
            Props::Browser(js) => {
                let q = if !js.contains('"') {
                    '"'
                } else if !js.contains('\'') {
                    '\''
                } else {
                    return Err(err(k, "<Island>'s props={:…} has both kinds of quote: make it a $derived in the script, props={:p}".into()));
                };
                out.push_str(&format!(" use:{comp}_mount={q}{js}{q}"));
            }
        }
        out.push('>');
        out.extend(src[k..tag.end].matches('\n'));
        let mut end = tag.end;
        if !tag.closed {
            let close = src[end..].find("</Island>").ok_or_else(|| {
                err(
                    k,
                    "<Island> has no </Island>; or close it: <Island … />".into(),
                )
            })?;
            out.push_str(&src[end..end + close]);
            end += close + "</Island>".len();
        }
        out.push_str("</div>");
        at = end;
        i = end;
        n += 1;
    }
    if n == 0 {
        return Ok(None);
    }
    out.push_str(&src[at..]);
    let mut code = format!(";{PROPS}");
    for (framework, mount) in MOUNTS {
        if used.contains(&framework) {
            code.push_str(mount);
        }
    }
    code.push_str(&js);
    match script {
        Some((close, ts)) => {
            if ts {
                code = code
                    .replace("(el, p)", "(el: any, p: any)")
                    .replace("(C)", "(C: any)")
                    .replace("(p) =>", "(p: any) =>");
            }
            // `close` is in `src`; the same place in `out` is as far from its end.
            let close = out.len() - (src.len() - close);
            let line = &out[out[..close].rfind('\n').map_or(0, |n| n + 1)..close];
            let lead = if line.trim().is_empty() { "" } else { "\n" };
            out.insert_str(close, &format!("{lead}{code}"));
        }
        None => out.push_str(&format!("\n<script>{code}</script>")),
    }
    Ok(Some(out))
}

/// What an `<Island>` tag says.
struct Tag {
    of: Option<String>,
    props: Props,
    /// The other attributes, for the `<div>`, each after a space.
    attrs: String,
    /// Just past the tag's `>`.
    end: usize,
    /// `/>`: no children.
    closed: bool,
}

enum Props {
    None,
    /// `props={rust}`: sent as JSON.
    Server(String),
    /// `props={:js}`: a browser value, kept current.
    Browser(String),
}

impl Tag {
    /// The tag's attributes from `i`, just past `<Island`. Errors carry
    /// their place.
    fn read(b: &[u8], mut i: usize) -> Result<Tag, (usize, String)> {
        let src = std::str::from_utf8(b).expect("from a str");
        let mut tag = Tag {
            of: None,
            props: Props::None,
            attrs: String::new(),
            end: 0,
            closed: false,
        };
        loop {
            while i < b.len() && b[i].is_ascii_whitespace() {
                i += 1;
            }
            match b.get(i) {
                None => return Err((i, "<Island> is not closed: <Island … />".into())),
                Some(b'>') => {
                    tag.end = i + 1;
                    return Ok(tag);
                }
                Some(b'/') if b.get(i + 1) == Some(&b'>') => {
                    tag.end = i + 2;
                    tag.closed = true;
                    return Ok(tag);
                }
                _ => {}
            }
            let at = i;
            while i < b.len() && !b[i].is_ascii_whitespace() && !matches!(b[i], b'=' | b'>' | b'/')
            {
                i += 1;
            }
            let name = &src[at..i];
            if name.is_empty() {
                return Err((
                    at,
                    "<Island> takes of=\"…\", props={:…} or props={…}, client:… and attributes"
                        .into(),
                ));
            }
            let mut j = i;
            while j < b.len() && b[j].is_ascii_whitespace() {
                j += 1;
            }
            let value = if b.get(j) == Some(&b'=') {
                j += 1;
                while j < b.len() && b[j].is_ascii_whitespace() {
                    j += 1;
                }
                let end = match b.get(j) {
                    Some(&q @ (b'"' | b'\'')) => {
                        b[j + 1..].iter().position(|&c| c == q).map(|e| j + e + 2)
                    }
                    Some(b'{') => crate::template::hole_end(b, j + 1).map(|e| e + 1),
                    _ => None,
                };
                let end = end.ok_or_else(|| {
                    (
                        at,
                        format!("`{name}=` needs a value: \"text\", {{rust}} or {{:js}}"),
                    )
                })?;
                i = end;
                Some(&src[j..end])
            } else {
                None
            };
            match (name, value) {
                ("of", Some(v)) if v.starts_with(['"', '\'']) => {
                    tag.of = Some(v[1..v.len() - 1].trim().to_string())
                }
                ("of", _) => return Err((at, "of= is text: of=\"react:react-switch\"".into())),
                ("props", Some(v)) if v.starts_with('{') => {
                    let inner = v[1..v.len() - 1].trim();
                    tag.props = match inner.strip_prefix(':') {
                        Some(js) => Props::Browser(js.trim().to_string()),
                        None => Props::Server(inner.to_string()),
                    };
                }
                ("props", _) => {
                    return Err((
                        at,
                        "props= is {:js} (a browser value) or {rust} (sent as JSON)".into(),
                    ));
                }
                _ => {
                    tag.attrs.push(' ');
                    tag.attrs.push_str(name);
                    if let Some(v) = value {
                        tag.attrs.push('=');
                        tag.attrs.push_str(v);
                    }
                }
            }
        }
    }
}

/// `react:pkg/sub#Name` → (`react`, `pkg/sub`, `Some("Name")`); without
/// `#Name`, the module's default export.
fn spec(of: &str) -> Result<(&'static str, &str, Option<&str>), String> {
    let names = || MOUNTS.iter().map(|m| m.0).collect::<Vec<_>>().join(", ");
    let Some((framework, rest)) = of.split_once(':') else {
        return Err(format!(
            "of=\"{of}\" names its framework first: of=\"react:{of}\" ({})",
            names()
        ));
    };
    let Some(&(framework, _)) = MOUNTS.iter().find(|m| m.0 == framework) else {
        return Err(format!(
            "`{framework}` is not a framework <Island> mounts: {}",
            names()
        ));
    };
    let (module, name) = match rest.split_once('#') {
        Some((m, n)) => (m, Some(n)),
        None => (rest, None),
    };
    let ident = |n: &str| {
        n.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_' || c == '$')
            && n.bytes()
                .all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'$')
    };
    if module.is_empty()
        || module.contains(['\'', '"', '\\'])
        || module.chars().any(char::is_whitespace)
    {
        return Err(format!(
            "of=\"{of}\" needs a module after `{framework}:`, like {framework}:some-package"
        ));
    }
    if name.is_some_and(|n| !ident(n)) {
        return Err(format!(
            "of=\"{of}\": after # comes the export's name, like #Chart"
        ));
    }
    Ok((framework, module, name))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_files_stay() {
        assert_eq!(expand("<p>Island</p><Islands />").unwrap(), None);
    }

    #[test]
    fn an_island_is_a_kept_div_that_mounts() {
        let src = "<h1>x</h1>\n<Island of=\"react:react-switch\" client:visible\n  props={:{ checked: on }} class=\"s\" />\n<p>after</p>\n<script>\n  let on = true\n</script>\n";
        let out = expand(src).unwrap().unwrap();
        assert!(
            out.contains("<div data-wisp-keep client:visible class=\"s\" use:__island0_mount=\"{ checked: on }\">\n</div>\n<p>after</p>"),
            "{out}"
        );
        // Lines kept: what follows is on its line, the script's code too.
        assert_eq!(out.lines().nth(3), Some("<p>after</p>"));
        assert!(out.contains("let on = true\n;const __props"), "{out}");
        assert!(out.ends_with("(__island0);</script>\n"), "{out}");
        assert!(out.contains("import __island0 from 'react-switch';const __island0_mount = __mount_react(__island0);"), "{out}");
        assert!(out.contains("from 'react-dom/client'"), "{out}");
        assert!(!out.contains("vue"), "{out}");
    }

    #[test]
    fn server_props_named_exports_and_children() {
        let src = "<Island of=\"vue:$lib/Chart.js#Chart\" props={rows}>Loading…</Island>";
        let out = expand(src).unwrap().unwrap();
        assert!(
            out.starts_with("<div data-wisp-keep data-props={::wisp::to_json(&(rows))} use:__island0_mount>Loading…</div>\n<script>"),
            "{out}"
        );
        assert!(
            out.contains("import { Chart as __island0 } from '$lib/Chart.js';"),
            "{out}"
        );
        assert!(out.contains("from 'vue'"), "{out}");
    }

    #[test]
    fn mistakes_are_named() {
        for (src, want) in [
            ("<Island />", "needs of="),
            ("<Island of=\"solid:x\" />", "not a framework"),
            ("<Island of=\"x\" />", "framework first"),
            ("<Island of=\"react:x#a-b\" />", "export's name"),
            ("<Island of=\"react:x\">", "no </Island>"),
            ("<Island of=\"react:x\" props=\"a\" />", "props= is"),
        ] {
            let e = expand(src).unwrap_err();
            assert!(e.msg.contains(want), "{src}: {}", e.msg);
        }
    }
}
