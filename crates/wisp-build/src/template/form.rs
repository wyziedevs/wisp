//! Rewrites before parsing: `{:@pager}`s and the `fields` of action forms.

use super::*;

/// `{@pager posts}` as the links to the pages either side of a
/// `Page` (`posts.prev`, `posts.next`), written out
/// on its line. `None` when there is none.
pub(super) fn pagers(src: &str) -> Option<String> {
    let (mut out, mut from) = (String::new(), 0);
    while let Some(at) = src[from..].find("{@pager") {
        let open = from + at;
        let arg = open + 7;
        let end = hole_end(src.as_bytes(), arg)?;
        let page = src[arg..end].trim();
        let simple = page
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'.' | b':'));
        let page = if simple {
            page.to_string()
        } else {
            format!("({page})")
        };
        out.push_str(&src[from..open]);
        for (side, text) in [("prev", "Newer"), ("next", "Older")] {
            out.push_str(&format!(
                "{{#if let Some(href) = &{page}.{side}}}<a {{href}}>{text}</a>{{/if}}"
            ));
        }
        from = end + 1;
    }
    (from > 0).then(|| out + &src[from..])
}

/// `<form action="?/add" fields>` with a labelled `<input>` for each
/// parameter of that action written in (`rules::input_type` gives its
/// `type`; a text field named like `body` is a `<textarea>`), all on the
/// tag's line, so the lines of the file stay where they are. A form with no
/// `action` or `method` posts to `fn default` (`method="post"` is added);
/// `fields={post}` starts each field of a struct parameter from `post`.
/// `None` when no form says `fields`.
pub(super) fn form_fields(src: &str, fields: &[Field]) -> Result<Option<String>, Error> {
    use std::fmt::Write as _;
    if !src.contains("fields") {
        return Ok(None);
    }
    let b = src.as_bytes();
    let (mut out, mut from, mut i) = (String::new(), 0, 0);
    while let Some(at) = src[i..].find("<form") {
        let start = i + at;
        i = start + 5;
        if !b.get(i).is_some_and(|&c| is_ws(c)) {
            continue;
        }
        // The tag's attributes: (name, value) and where each starts and ends.
        let (mut attrs, mut j) = (Vec::new(), i);
        loop {
            while j < b.len() && is_ws(b[j]) {
                j += 1;
            }
            if j >= b.len() || b[j] == b'>' || src[j..].starts_with("/>") {
                break;
            }
            let at = j;
            while j < b.len() && !is_ws(b[j]) && !matches!(b[j], b'=' | b'>') {
                j += 1;
            }
            let name = &src[at..j];
            let mut value = "";
            if b.get(j) == Some(&b'=') {
                j += 1;
                let from = j;
                let quote = b.get(j).copied().filter(|&q| q == b'"' || q == b'\'');
                j += usize::from(quote.is_some());
                while j < b.len() && quote.map_or(!is_ws(b[j]) && b[j] != b'>', |q| b[j] != q) {
                    j = if b[j] == b'{' {
                        hole_end(b, j + 1).map_or(b.len(), |e| e + 1)
                    } else {
                        j + 1
                    };
                }
                value = &src[from + usize::from(quote.is_some())..j.min(b.len())];
                j += usize::from(quote.is_some());
            }
            attrs.push((name, value, at, j.min(b.len())));
        }
        let Some(&(_, given, tok, tok_end)) = attrs.iter().find(|a| {
            a.0 == "fields"
                && (a.1.is_empty() && !src[a.2..a.3].contains('=')
                    || a.1.starts_with('{') && a.1.ends_with('}'))
        }) else {
            continue;
        };
        let start_from = given.strip_prefix('{').and_then(|v| v.strip_suffix('}'));
        let value = |n: &str| attrs.iter().find(|a| a.0 == n).map(|a| a.1);
        let mut implied = false;
        let action = match (value("action"), value("method")) {
            (Some(a), _) => a
                .strip_prefix("?/")
                .map(|a| a.split('&').next().unwrap_or(a)),
            (None, Some(m)) if m.eq_ignore_ascii_case("post") => Some("default"),
            (None, None) => {
                implied = true;
                Some("default")
            }
            _ => None,
        };
        let line = src[..start].matches('\n').count() as u32 + 1;
        let col = (start - src[..start].rfind('\n').map_or(0, |n| n + 1)) as u32 + 1;
        let fail = |msg: &str| Error {
            line,
            col,
            msg: msg.into(),
        };
        let action = action
            .filter(|a| !a.contains('{'))
            .ok_or_else(|| fail("`fields` needs the form to post to a named action: `<form action=\"?/add\" fields>`"))?;
        let mut inputs = String::new();
        for f in fields.iter().filter(|f| f.action == action) {
            let kind = crate::rules::input_type(&f.name, &f.ty);
            let mut label = f.name.replace('_', " ");
            if let Some(first) = label.get_mut(..1) {
                first.make_ascii_uppercase();
            }
            let from = (start_from.filter(|_| f.whole)).map(|e| format!("{e}.{}", f.name));
            let _ = if kind.is_empty() && crate::rules::is_long(&f.name, &f.ty) {
                let text = from.map(|e| format!("{{{e}}}")).unwrap_or_default();
                write!(
                    inputs,
                    "<label>{label} <textarea name=\"{}\">{text}</textarea></label>",
                    f.name
                )
            } else {
                let typed = if kind.is_empty() {
                    String::new()
                } else {
                    format!(" type=\"{kind}\"")
                };
                let start = match (kind, from) {
                    ("password" | "file", _) | (_, None) => String::new(),
                    ("checkbox", Some(e)) => format!(" checked={{{e}}}"),
                    (_, Some(e)) => format!(" value={{{e}}}"),
                };
                write!(
                    inputs,
                    "<label>{label} <input name=\"{}\"{typed}{start}></label>",
                    f.name
                )
            };
        }
        if inputs.is_empty() {
            return Err(fail(if implied {
                "`<form fields>` posts to `fn default`, and this page has none with parameters: write one, or `action=\"?/name\"`"
            } else {
                "`fields`: this action takes nothing a form could ask for (a `#[action]` of this page, with parameters)"
            }));
        }
        let end = (j + 1 + usize::from(src[j..].starts_with("/>"))).min(src.len());
        out.push_str(&src[from..src[..tok].trim_end().len()]);
        if implied {
            out.push_str(" method=\"post\"");
        }
        out.push_str(&src[tok_end..end]);
        out.push_str(&inputs);
        (from, i) = (end, end);
    }
    out.push_str(&src[from..]);
    Ok((from > 0).then_some(out))
}
