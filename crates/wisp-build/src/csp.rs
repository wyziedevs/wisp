//! Content Security Policy: the hashes of the inline scripts the app's
//! templates and shell write, which the runtime's `script-src` allows
//! (wisp's `csp.rs`). A `<script>`'s text has no holes, so its hash is
//! known at build and costs nothing per request, holds for baked and
//! `CACHE` pages, and for the scripts wisp.js runs after a navigation.

use wisp_shared::{base64, sha256::sha256};

/// `'sha256-…'`, a script's text as a CSP source.
pub fn hash(js: &str) -> String {
    let mut out = String::from("'sha256-");
    base64::encode(&mut out, &sha256(&[js.as_bytes()]), false);
    out.push('\'');
    out
}

/// Whether a `<script>` runs its own text: it has no `src`, and its `type`
/// is none, JavaScript, a module or an import map.
pub fn runs(src: bool, ty: Option<&str>) -> bool {
    let ty = ty
        .map(|t| t.trim().to_ascii_lowercase())
        .unwrap_or_default();
    !src && (ty.is_empty()
        || ty == "module"
        || ty == "importmap"
        || ty.ends_with("/javascript")
        || ty.ends_with("/ecmascript"))
}

/// The hashes of the scripts that run in plain HTML (the shell's), onto
/// `out`. Comments are skipped.
pub fn in_html(html: &str, out: &mut Vec<String>) {
    let lower = html.to_ascii_lowercase();
    let mut i = 0;
    while let Some(at) = lower[i..].find('<').map(|k| i + k) {
        if lower[at..].starts_with("<!--") {
            i = lower[at..].find("-->").map_or(html.len(), |k| at + k + 3);
            continue;
        }
        i = at + 1;
        let tag = &lower[at..];
        let ends = |c: char| c.is_ascii_whitespace() || c == '>' || c == '/';
        if !tag.starts_with("<script") || !tag[7..].starts_with(ends) {
            continue;
        }
        let (mut src, mut ty, mut k) = (false, None, at + 7);
        // The attributes, to the tag's end.
        let b = lower.as_bytes();
        while k < b.len() && b[k] != b'>' {
            if b[k].is_ascii_whitespace() || b[k] == b'/' {
                k += 1;
                continue;
            }
            let name_at = k;
            while k < b.len() && !b[k].is_ascii_whitespace() && !matches!(b[k], b'=' | b'>' | b'/')
            {
                k += 1;
            }
            let name = &lower[name_at..k];
            let mut value = None;
            if b.get(k) == Some(&b'=') {
                k += 1;
                let (from, to) = match b.get(k) {
                    Some(&q @ (b'"' | b'\'')) => {
                        let end = lower[k + 1..]
                            .find(q as char)
                            .map_or(b.len(), |e| k + 1 + e);
                        (k + 1, end)
                    }
                    _ => {
                        let end = (k..b.len())
                            .find(|&e| b[e].is_ascii_whitespace() || b[e] == b'>')
                            .unwrap_or(b.len());
                        (k, end)
                    }
                };
                value = Some(&html[from..to]);
                k = (to + 1).min(b.len());
            }
            match name {
                "src" => src = true,
                "type" => ty = value,
                _ => {}
            }
        }
        let body = (k + 1).min(html.len());
        let Some(end) = lower[body..].find("</script").map(|e| body + e) else {
            return;
        };
        if runs(src, ty) {
            out.push(hash(&html[body..end]));
        }
        i = end;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hashes_are_csp_sources() {
        // `echo -n 'alert(1)' | openssl dgst -sha256 -binary | base64`
        assert_eq!(
            hash("alert(1)"),
            "'sha256-bhHHL3z2vDgxUt0W3dWQOrprscmda2Y5pLsLg4GF+pI='"
        );
    }

    #[test]
    fn only_scripts_that_run() {
        assert!(runs(false, None) && runs(false, Some(" Module")));
        assert!(runs(false, Some("text/javascript")) && runs(false, Some("importmap")));
        assert!(!runs(true, None) && !runs(false, Some("application/ld+json")));
        let mut out = Vec::new();
        in_html(
            "<!-- <script>no()</script> --><SCRIPT>a()</SCRIPT><script src=\"x.js\"></script>\
             <script type='application/json'>{}</script><scripts></scripts>\
             <script type=module data-x=\"a>b\">b()</script>",
            &mut out,
        );
        assert_eq!(out, [hash("a()"), hash("b()")]);
    }
}
