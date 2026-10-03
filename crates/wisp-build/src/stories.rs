//! `Card.stories.wisp`, beside `Card.wisp`: named examples of a component,
//! `{#story "Featured"}<Card featured title="x" />{/story}`, for the
//! workshop at `/_wisp/components`. Dev builds only: a release build and
//! the routes never read these files.
//!
//! Each story is markup of its own, rendered on the server. Its
//! component's simple props (text, numbers, `bool`) read the query first,
//! so the workshop's controls re-render it with other values.

use crate::template::{Code, Node, Prop, PropDecl, PropValue};

/// The suffix of a stories file.
pub const SUFFIX: &str = ".stories.wisp";

#[derive(Debug)]
pub struct Story {
    pub name: String,
    /// For its URL: `featured-card`.
    pub slug: String,
    pub line: u32,
    /// The body, after as many line breaks as come before it in the file,
    /// so the parser's lines are the file's.
    pub markup: String,
}

/// The `{#story "Name"}…{/story}` blocks of a stories file. Outside them
/// only whitespace and comments may be. Errors start with their line.
pub fn split(src: &str) -> Result<Vec<Story>, String> {
    let line_at = |at: usize| src[..at].matches('\n').count() as u32 + 1;
    let mut out: Vec<Story> = Vec::new();
    let mut at = 0;
    loop {
        let open = src[at..].find("{#story").map(|i| at + i);
        let gap = &src[at..open.unwrap_or(src.len())];
        if let Some(i) = stray(gap) {
            return Err(format!(
                "{}: only {{#story \"Name\"}}…{{/story}} blocks go in a stories file",
                line_at(at + i)
            ));
        }
        let Some(open) = open else {
            return Ok(out);
        };
        let line = line_at(open);
        let head = src[open + "{#story".len()..].trim_start();
        let name = head
            .strip_prefix('"')
            .and_then(|h| h.split_once('"'))
            .filter(|(n, rest)| !n.trim().is_empty() && rest.trim_start().starts_with('}'));
        let Some((name, rest)) = name else {
            return Err(format!(
                "{line}: a story is {{#story \"Name\"}}…{{/story}}, its name in double quotes"
            ));
        };
        let body_at = src.len() - rest.trim_start().len() + 1;
        let Some(close) = src[body_at..].find("{/story}").map(|i| body_at + i) else {
            return Err(format!("{line}: {{#story \"{name}\"}} has no {{/story}}"));
        };
        let slug = slug(name);
        if out.iter().any(|s| s.slug == slug) {
            return Err(format!("{line}: there is already a story \"{name}\" here"));
        }
        let mut markup = "\n".repeat(line_at(body_at) as usize - 1);
        markup.push_str(&src[body_at..close]);
        out.push(Story {
            name: name.trim().to_string(),
            slug,
            line,
            markup,
        });
        at = close + "{/story}".len();
    }
}

/// Where in `gap` something other than whitespace and `<!-- -->` is.
fn stray(gap: &str) -> Option<usize> {
    let mut at = 0;
    while at < gap.len() {
        let rest = &gap[at..];
        let c = rest.chars().next()?;
        if rest.starts_with("<!--") {
            at += rest.find("-->").map_or(rest.len(), |i| i + 3);
        } else if c.is_whitespace() {
            at += c.len_utf8();
        } else {
            return Some(at);
        }
    }
    None
}

/// `Big Card!` is `big-card`.
fn slug(name: &str) -> String {
    let mut s = String::new();
    for c in name.trim().chars() {
        if c.is_ascii_alphanumeric() {
            s.push(c.to_ascii_lowercase());
        } else if !s.ends_with('-') {
            s.push('-');
        }
    }
    let s = s.trim_matches('-');
    if s.is_empty() {
        "story".into()
    } else {
        s.into()
    }
}

/// How the workshop edits a prop of this Rust type, if it can.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Control {
    Text,
    Number,
    Check,
}

impl Control {
    pub fn of(ty: &str) -> Option<Control> {
        match ty {
            "&str" | "String" => Some(Control::Text),
            "bool" => Some(Control::Check),
            "u8" | "u16" | "u32" | "u64" | "u128" | "usize" | "i8" | "i16" | "i32" | "i64"
            | "i128" | "isize" | "f32" | "f64" => Some(Control::Number),
            _ => None,
        }
    }

    /// Its name in `wisp::rt::Control`.
    pub fn rust(self) -> &'static str {
        match self {
            Control::Text => "Text",
            Control::Number => "Number",
            Control::Check => "Check",
        }
    }

    /// What a required prop the story leaves out starts as.
    fn blank(self, name: &str) -> String {
        match self {
            Control::Text => format!("{name:?}"),
            Control::Number => "0".into(),
            Control::Check => "false".into(),
        }
    }
}

/// Gives every `<comp>` in `nodes` its simple props from the query, with
/// what the story wrote (or the prop's default) when the query has none.
/// `fill`: a required one the story leaves out starts blank (the default
/// story's). The values the first `<comp>` starts with, where they are
/// literals, for the controls.
pub fn wire(
    nodes: &mut [Node],
    comp: &str,
    decls: &[PropDecl],
    fill: bool,
) -> Vec<(String, String)> {
    let mut first: Option<Vec<(String, String)>> = None;
    for n in nodes {
        let Node::Component {
            name,
            props,
            children,
            line,
        } = n
        else {
            continue;
        };
        if let Some(kids) = children {
            let inner = wire(kids, comp, decls, fill);
            if first.is_none() && !inner.is_empty() {
                first = Some(inner);
            }
        }
        if name != comp {
            continue;
        }
        let mut values = Vec::new();
        for d in decls {
            let Some(c) = Control::of(&d.ty) else {
                continue;
            };
            let at = props.iter().position(|p| p.name == d.name);
            let start = match at.map(|i| &props[i].value) {
                Some(PropValue::Text(t)) if c == Control::Text => format!("{t:?}"),
                Some(PropValue::Flag) if c == Control::Check => "true".into(),
                Some(PropValue::Expr(e)) => e.src.trim().to_string(),
                Some(_) => continue,
                None => match &d.default {
                    Some(v) => v.trim().to_string(),
                    None if fill => c.blank(&d.name),
                    None => continue,
                },
            };
            let src = match c {
                Control::Text => format!(
                    "cx.query_or::<String>({:?}, ::std::string::ToString::to_string(&({start})))",
                    d.name
                ),
                _ => format!("cx.query_or({:?}, {start})", d.name),
            };
            let value = PropValue::Expr(Code { src, line: *line });
            match at {
                Some(i) => props[i].value = value,
                None => props.push(Prop {
                    name: d.name.clone(),
                    value,
                }),
            }
            if let Some(v) = literal(&start) {
                values.push((d.name.clone(), v));
            }
        }
        if first.is_none() {
            first = Some(values);
        }
    }
    first.unwrap_or_default()
}

/// A literal's value as a control shows it: `"x"` is `x`, `3` and `true`
/// are themselves. `None` for anything worked out.
fn literal(src: &str) -> Option<String> {
    if let Some(s) = src.strip_prefix('"').and_then(|s| s.strip_suffix('"')) {
        return (!s.contains(['"', '\\'])).then(|| s.to_string());
    }
    let number = src.parse::<f64>().is_ok() && !src.contains(['e', 'E', 'i', 'n']);
    (number || matches!(src, "true" | "false")).then(|| src.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stories_split_with_their_lines() {
        let src = "<!-- cards -->\n{#story \"Featured\"}\n<Card featured title=\"x\" />\n{/story}\n\n{#story \"Big one\"}<Card title=\"y\" />{/story}\n";
        let s = split(src).unwrap();
        assert_eq!(s.len(), 2);
        assert_eq!(
            (s[0].name.as_str(), s[0].slug.as_str(), s[0].line),
            ("Featured", "featured", 2)
        );
        assert_eq!(s[0].markup, "\n\n<Card featured title=\"x\" />\n");
        assert_eq!((s[1].slug.as_str(), s[1].line), ("big-one", 6));
        assert!(s[1].markup.starts_with("\n\n\n\n\n<Card"));
        assert_eq!(split("").unwrap().len(), 0);
        for (bad, why) in [
            ("<p>hi</p>", "1: only"),
            ("{#story Featured}x{/story}", "1: a story is"),
            ("\n{#story \"A\"}x", "2: {#story \"A\"} has no"),
            ("{#story \"A\"}x{/story}{#story \"a\"}y{/story}", "already"),
        ] {
            let e = split(bad).unwrap_err();
            assert!(e.starts_with(why) || e.contains(why), "{bad}: {e}");
        }
    }

    #[test]
    fn simple_props_read_the_query() {
        let decl = |name: &str, ty: &str, default: Option<&str>| PropDecl {
            name: name.into(),
            ty: ty.into(),
            default: default.map(Into::into),
        };
        let decls = [
            decl("title", "&str", None),
            decl("count", "u32", Some("0")),
            decl("featured", "bool", Some("false")),
            decl("rows", "&[u8]", None),
        ];
        let t = crate::template::parse_with(
            "<Card featured title=\"x\" rows={&[]} />",
            &[],
            "w-t",
            false,
        )
        .unwrap();
        let mut nodes = t.nodes;
        let values = wire(&mut nodes, "Card", &decls, false);
        assert_eq!(
            values,
            [
                ("title".into(), "x".into()),
                ("count".into(), "0".into()),
                ("featured".into(), "true".into())
            ]
        );
        let props = nodes
            .iter()
            .find_map(|n| match n {
                Node::Component { props, .. } => Some(props),
                _ => None,
            })
            .unwrap();
        let src = |n: &str| match &props.iter().find(|p| p.name == n).unwrap().value {
            PropValue::Expr(c) => c.src.clone(),
            v => format!("{v:?}"),
        };
        assert_eq!(src("featured"), "cx.query_or(\"featured\", true)");
        assert_eq!(src("count"), "cx.query_or(\"count\", 0)");
        assert!(src("title").starts_with("cx.query_or::<String>(\"title\""));
        assert_eq!(src("rows"), "&[]");
        // The default story fills in a required prop.
        let mut nodes = crate::template::parse_with("<Card />", &[], "w-t", false)
            .unwrap()
            .nodes;
        let values = wire(&mut nodes, "Card", &decls, true);
        assert_eq!(values[0], ("title".into(), "title".into()));
    }
}
