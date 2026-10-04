//! Open Graph images made at build time: `wisp::og("Title", "About it",
//! "auto")` in a page names `/og/<slug>.svg`, and `wisp build` writes that
//! file into `static/og/` for each such call whose title and description are
//! literals in a template. The same functions serve the runtime (the URL)
//! and the build (the picture). The picture is SVG: no rasterizer is in
//! Wisp's dependencies, and many crawlers want PNG (docs/design.md says how
//! to convert).

/// What the picture is drawn with: CSS colors and the name shown in a corner.
pub struct Look<'a> {
    pub bg: &'a str,
    pub ink: &'a str,
    pub accent: &'a str,
    pub brand: &'a str,
}

impl Look<'_> {
    /// Wisp's own colors (kinetrix, violet accent).
    pub const DEFAULT: Look<'static> = Look {
        bg: "#0b0b0f",
        ink: "#fafafa",
        accent: "#8b5cf6",
        brand: "",
    };
}

/// The name of a title's file: lowercase letters and digits, runs of
/// anything else one `-`, at most 60 bytes (`page` for nothing).
pub fn slug(title: &str) -> String {
    let mut out = String::new();
    for c in title.chars().flat_map(char::to_lowercase) {
        if c.is_ascii_alphanumeric() {
            out.push(c);
        } else if !out.is_empty() && !out.ends_with('-') {
            out.push('-');
        }
    }
    out.truncate(60);
    let out = out.trim_end_matches('-');
    if out.is_empty() {
        "page".into()
    } else {
        out.into()
    }
}

/// Where the picture of `title` is served.
pub fn url(title: &str) -> String {
    format!("/og/{}.svg", slug(title))
}

/// `text` in lines of at most `n` characters, at most `max` of them (the
/// last ends in an ellipsis when the text goes on).
fn wrap(text: &str, n: usize, max: usize) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    for word in text.split_whitespace() {
        match lines.last_mut() {
            Some(l) if l.chars().count() + 1 + word.chars().count() <= n => {
                l.push(' ');
                l.push_str(word);
            }
            _ => lines.push(word.to_string()),
        }
    }
    if lines.len() > max {
        lines.truncate(max);
        let last = &mut lines[max - 1];
        *last = last.chars().take(n.saturating_sub(1)).collect::<String>() + "…";
    }
    lines
}

fn escape(s: &str) -> String {
    (s.replace('&', "&amp;"))
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// The 1200 by 630 picture: title large, description under it, the brand in
/// the corner, an accent bar.
pub fn svg(title: &str, description: &str, look: &Look) -> String {
    let mut out = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"1200\" height=\"630\" viewBox=\"0 0 1200 630\">\n\
         <rect width=\"1200\" height=\"630\" fill=\"{}\"/>\n<rect width=\"16\" height=\"630\" fill=\"{}\"/>\n\
         <g font-family=\"system-ui, -apple-system, Segoe UI, Roboto, sans-serif\" fill=\"{}\">\n",
        escape(look.bg),
        escape(look.accent),
        escape(look.ink)
    );
    let mut y = 150;
    for l in wrap(title, 26, 3) {
        out.push_str(&format!(
            "<text x=\"80\" y=\"{y}\" font-size=\"64\" font-weight=\"700\">{}</text>\n",
            escape(&l)
        ));
        y += 80;
    }
    y += 20;
    for l in wrap(description, 52, 3) {
        out.push_str(&format!(
            "<text x=\"80\" y=\"{y}\" font-size=\"32\" opacity=\"0.7\">{}</text>\n",
            escape(&l)
        ));
        y += 46;
    }
    if !look.brand.is_empty() {
        out.push_str(&format!(
            "<text x=\"80\" y=\"570\" font-size=\"30\" font-weight=\"600\" fill=\"{}\">{}</text>\n",
            escape(look.accent),
            escape(look.brand)
        ));
    }
    out.push_str("</g>\n</svg>\n");
    out
}

/// The value of custom property `--name` in `css`, the first.
pub fn token(css: &str, name: &str) -> Option<String> {
    let at = css.find(&format!("--{name}:"))? + name.len() + 3;
    let end = css[at..].find([';', '}'])?;
    let v = css[at..at + end].trim();
    (!v.is_empty() && !v.contains("var(")).then(|| v.to_string())
}

/// A Rust string literal at the start of `s` (after spaces): its text, and
/// what follows. Only `\"`, `\\` and `\n` are understood.
fn literal(s: &str) -> Option<(String, &str)> {
    let s = s.trim_start().strip_prefix('"')?;
    let mut out = String::new();
    let mut chars = s.char_indices();
    while let Some((i, c)) = chars.next() {
        match c {
            '"' => return Some((out, &s[i + 1..])),
            '\\' => match chars.next()?.1 {
                'n' => out.push('\n'),
                e @ ('"' | '\\') => out.push(e),
                _ => return None,
            },
            c => out.push(c),
        }
    }
    None
}

/// The (title, description) of each `wisp::og("…", "…", "auto")` in `src`
/// whose first two are literals.
pub fn calls(src: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for (at, _) in src.match_indices("og(") {
        if !src[..at].ends_with("wisp::") && !src[..at].ends_with("{@html ") {
            continue;
        }
        let parsed = (|| {
            let (title, rest) = literal(&src[at + 3..])?;
            let (desc, rest) = literal(rest.trim_start().strip_prefix(',')?)?;
            let (image, _) = literal(rest.trim_start().strip_prefix(',')?)?;
            (image == "auto").then_some((title, desc))
        })();
        out.extend(parsed);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_and_pictures() {
        assert_eq!(slug("Hello, World! 2"), "hello-world-2");
        assert_eq!(slug("  --  "), "page");
        assert_eq!(url("A b"), "/og/a-b.svg");
        let look = Look {
            brand: "Acme & Co",
            ..Look::DEFAULT
        };
        let long = "A <very> long title that needs more than one line to fit";
        let s = svg(long, "d", &look);
        assert!(s.contains("&lt;very&gt;") && s.contains("Acme &amp; Co"));
        assert!(s.matches("<text").count() >= 4);
        assert!(s.starts_with("<svg") && s.ends_with("</svg>\n"));
    }

    #[test]
    fn finds_literal_calls_and_tokens() {
        let src = "wisp::og(\"Hi \\\"x\\\"\", \"About\", \"auto\")} wisp::og(t, \"d\", \"auto\") wisp::og(\"A\", \"B\", \"/c.png\")";
        assert_eq!(calls(src), [("Hi \"x\"".to_string(), "About".to_string())]);
        let css = ":root { --bg: #fff; --accent : red; --x: var(--bg); }";
        assert_eq!(token(css, "bg").as_deref(), Some("#fff"));
        assert_eq!(token(css, "x"), None);
    }
}
