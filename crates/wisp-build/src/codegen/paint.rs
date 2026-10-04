//! First paint: the server paints what the browser will keep drawing.

use super::*;

// ---- first paint ------------------------------------------------------------
//
// The server paints client blocks, components and `{:…}` whose values it
// knows, so the page shows them before (and without) JavaScript, and the
// browser takes those nodes over. It knows server values (`data`, props,
// Rust loop names), literals, script variables first set to those, and the
// item and index of a client `each` it is painting; `!`, `&&`, `||` and
// `.length` of those. Anything else (a call, a sum, a comparison) is left
// to the browser.

/// A browser value the server works out, as the Rust expression that does:
/// a `wisp::rt::Js`, a length or index (`Option<usize>`), a `bool`, or a
/// `&&`/`||` (a `bool` that only decides: its JavaScript value is an operand).
#[derive(Clone, Debug)]
pub(super) enum Pv {
    Val(String),
    Num(String),
    Bool(String),
    Test(String),
}

impl Pv {
    /// As `if (x)` tests it.
    pub(super) fn test(&self) -> String {
        match self {
            Pv::Val(e) => format!("{e}.truthy()"),
            Pv::Num(e) => format!("{e}.is_some_and(|n| n > 0)"),
            Pv::Bool(e) | Pv::Test(e) => e.clone(),
        }
    }

    /// As a `Js` (a component's prop).
    pub(super) fn val(&self) -> Option<String> {
        match self {
            Pv::Val(e) => Some(e.clone()),
            Pv::Num(e) => Some(format!("::wisp::rt::Js(&::wisp::rt::js_of(&{e}))")),
            Pv::Bool(e) => Some(format!(
                "::wisp::rt::Js(if {e} {{ \"true\" }} else {{ \"false\" }})"
            )),
            Pv::Test(_) => None,
        }
    }

    /// The statement that writes it as `{:x}` shows it.
    pub(super) fn text(&self, buf: &str) -> Option<String> {
        match self {
            Pv::Val(e) => Some(format!("{e}.text(&mut {buf});")),
            Pv::Num(e) | Pv::Bool(e) => Some(format!("::wisp::rt::js_text(&mut {buf}, &{e});")),
            Pv::Test(_) => None,
        }
    }

    /// `x.a.b`, or `x.length`.
    pub(super) fn member(self, rest: &[String]) -> Option<Pv> {
        let mut v = self;
        for (k, seg) in rest.iter().enumerate() {
            v = match v {
                Pv::Val(e) if seg == "length" && k + 1 == rest.len() => {
                    Pv::Num(format!("{e}.length()"))
                }
                Pv::Val(e) => Pv::Val(format!("{e}.get({})", lit(seg))),
                _ => return None,
            };
        }
        Some(v)
    }
}

/// The browser value `js`, read by group `group`, if the server knows it.
pub(super) fn paint_value(cx: &Emit, group: usize, js: &str) -> Option<Pv> {
    let c = cx.client?;
    if cx.inert {
        return None;
    }
    paint_expr(js, &mut |path| resolve(cx, c, Some(group), path, 0))
}

/// A variable's path as the first paint knows it, read in `group`, or in
/// the script (`None`), which sees only server values and the script's
/// own variables. `depth` stops variables set from each other in a circle.
pub(super) fn resolve(
    cx: &Emit,
    c: &Client,
    group: Option<usize>,
    path: &[String],
    depth: u32,
) -> Option<Pv> {
    let name = &path[0];
    let env = &cx.env[..if group.is_some() {
        cx.env.len()
    } else {
        cx.props
    }];
    if let Some((_, v)) = env.iter().rev().find(|(n, _)| n == name) {
        return v.clone().member(&path[1..]);
    }
    if c.declared.contains(name) {
        let (_, init) = c
            .lets
            .iter()
            .find(|(n, _)| n == name)
            .filter(|_| depth < 8)?;
        return paint_expr(init, &mut |p| resolve(cx, c, None, p, depth + 1))?.member(&path[1..]);
    }
    // A client local the server is not painting.
    if group.is_some_and(|g| cx.template.groups[g].locals.contains(name)) {
        return None;
    }
    let rust = group.is_some_and(|g| c.scopes[g].contains(name));
    if !rust && !(c.server.contains(name) && !cx.paint) {
        return None;
    }
    let place = if c.whole && !rust {
        path[..1].to_vec()
    } else {
        server_path(path)
    };
    Pv::Val(format!(
        "::wisp::rt::Js(&::wisp::rt::js_of(&({})))",
        rust_place(&place)
    ))
    .member(&path[place.len()..])
}

/// `js` worked out by the server, if it can be: see `Pv`. `root` resolves a
/// variable's path (`data.user.name` whole).
pub(super) fn paint_expr(js: &str, root: &mut dyn FnMut(&[String]) -> Option<Pv>) -> Option<Pv> {
    let t = js::tokens(js);
    let mut k = 0;
    let v = paint_or(js, &t, &mut k, root)?;
    (k == t.len()).then_some(v)
}

pub(super) type Root<'a> = dyn FnMut(&[String]) -> Option<Pv> + 'a;

pub(super) fn paint_or(js: &str, t: &[js::Token], k: &mut usize, root: &mut Root) -> Option<Pv> {
    let mut v = paint_and(js, t, k, root)?;
    while t.get(*k).is_some_and(|n| n.text(js) == "||") {
        *k += 1;
        let w = paint_and(js, t, k, root)?;
        v = Pv::Test(format!("({} || {})", v.test(), w.test()));
    }
    Some(v)
}

pub(super) fn paint_and(js: &str, t: &[js::Token], k: &mut usize, root: &mut Root) -> Option<Pv> {
    let mut v = paint_not(js, t, k, root)?;
    while t.get(*k).is_some_and(|n| n.text(js) == "&&") {
        *k += 1;
        let w = paint_not(js, t, k, root)?;
        v = Pv::Test(format!("({} && {})", v.test(), w.test()));
    }
    Some(v)
}

pub(super) fn paint_not(js: &str, t: &[js::Token], k: &mut usize, root: &mut Root) -> Option<Pv> {
    let n = *t.get(*k)?;
    let text = n.text(js);
    match (n.kind, text) {
        (JsKind::Punct, "!") => {
            *k += 1;
            let v = paint_not(js, t, k, root)?;
            Some(Pv::Bool(format!("!({})", v.test())))
        }
        (JsKind::Punct, "(") => {
            *k += 1;
            let v = paint_or(js, t, k, root)?;
            (t.get(*k)?.text(js) == ")").then(|| *k += 1)?;
            Some(v)
        }
        (JsKind::Punct, "[" | "{") | (JsKind::Number | JsKind::String, _) => {
            let end = if n.kind == JsKind::Punct {
                (*k + 1..t.len()).find(|&j| t[j].depth <= n.depth)?
            } else {
                *k
            };
            if let Some(json) = literal_json(js, &t[*k..=end]) {
                *k = end + 1;
                return Some(Pv::Val(format!("::wisp::rt::Js({})", lit(&json))));
            }
            paint_composite(js, t, k, end, root)
        }
        (JsKind::Ident, "true" | "false" | "null" | "undefined") => {
            *k += 1;
            let json = if text == "undefined" { "null" } else { text };
            Some(Pv::Val(format!("::wisp::rt::Js({})", lit(json))))
        }
        (JsKind::Ident, _) if !js::is_reserved(text) && !text.starts_with('#') => {
            let mut path = vec![text.to_string()];
            *k += 1;
            while t
                .get(*k)
                .is_some_and(|n| n.kind == JsKind::Punct && n.text(js) == ".")
                && t.get(*k + 1)
                    .is_some_and(|n| n.kind == JsKind::Ident && !n.text(js).starts_with('#'))
            {
                path.push(t[*k + 1].text(js).to_string());
                *k += 2;
            }
            root(&path)
        }
        _ => None,
    }
}

/// A JavaScript literal as JSON: numbers, strings, `true`, `false`, `null`,
/// and arrays and objects of them.
/// An array or object literal (`[a, 'b']`, `{ color: c, on }`) from `t[*k]`
/// to its end, whose values the first paint knows: its JSON, built when
/// the page renders.
pub(super) fn paint_composite(
    js: &str,
    t: &[js::Token],
    k: &mut usize,
    end: usize,
    root: &mut Root,
) -> Option<Pv> {
    let array = t[*k].text(js) == "[";
    let (mut fmt, mut args) = (String::from(if array { "[" } else { "{{" }), Vec::new());
    let mut j = *k + 1;
    while j < end {
        if !args.is_empty() {
            fmt.push(',');
        }
        if !array {
            // `key: value`, `'key': value` or `name` for `name: name`.
            let n = t[j];
            let key = match n.kind {
                JsKind::Ident => n.text(js).to_string(),
                JsKind::String => js_string(n.text(js))?,
                _ => return None,
            };
            let _ = write!(
                fmt,
                "{}:",
                js_str(&key).replace('{', "{{").replace('}', "}}")
            );
            if t.get(j + 1).is_some_and(|x| x.text(js) == ":") {
                j += 2;
            } else if n.kind != JsKind::Ident {
                return None;
            }
        }
        let v = paint_or(js, t, &mut j, root)?.val()?;
        fmt.push_str("{}");
        args.push(format!("({v}).0"));
        match t.get(j).map(|x| x.text(js)) {
            Some(",") if j < end => j += 1,
            _ if j == end => {}
            _ => return None,
        }
    }
    fmt.push_str(if array { "]" } else { "}}" });
    *k = end + 1;
    Some(Pv::Val(format!(
        "::wisp::rt::Js(&format!({}{}))",
        lit(&fmt),
        args.iter().map(|a| format!(", {a}")).collect::<String>()
    )))
}

pub(super) fn literal_json(js: &str, t: &[js::Token]) -> Option<String> {
    let mut out = String::new();
    for (k, n) in t.iter().enumerate() {
        let text = n.text(js);
        match n.kind {
            JsKind::Punct => match text {
                "[" | "{" | ":" | "," => out.push_str(text),
                "]" | "}" => {
                    if out.ends_with(',') {
                        out.pop(); // a trailing comma
                    }
                    out.push_str(text);
                }
                "-" if t.get(k + 1).is_some_and(|m| m.kind == JsKind::Number) => out.push('-'),
                _ => return None,
            },
            // As JavaScript shows it: `1.0` is `1`.
            JsKind::Number => {
                let x = text.parse::<f64>().ok().filter(|x| x.is_finite())?;
                let _ = write!(out, "{x}");
            }
            JsKind::String => out.push_str(&js_str(&js_string(text)?)),
            JsKind::Ident if n.key => out.push_str(&js_str(text)),
            JsKind::Ident if matches!(text, "true" | "false" | "null") => out.push_str(text),
            _ => return None,
        }
    }
    Some(out)
}

/// The text of a JavaScript string literal (`'a\'b'`), for the simple
/// escapes; `None` for others.
pub(super) fn js_string(lit: &str) -> Option<String> {
    let q = lit.chars().next()?;
    let inner = lit.strip_prefix(q)?.strip_suffix(q)?;
    let mut s = String::with_capacity(inner.len());
    let mut chars = inner.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            s.push(c);
            continue;
        }
        s.push(match chars.next()? {
            'n' => '\n',
            't' => '\t',
            'r' => '\r',
            c @ ('\\' | '\'' | '"') => c,
            _ => return None,
        });
    }
    Some(s)
}

pub(super) struct Emit<'a> {
    pub(super) rel: &'a str,
    pub(super) template: &'a Template,
    pub(super) comps: &'a [Comp],
    /// The path from the template's module to the top of the generated
    /// file, where the components are.
    pub(super) top: &'static str,
    pub(super) target: &'static str,
    pub(super) each_depth: usize,
    pub(super) client: Option<&'a Client>,
    /// What the first paint knows by name: a painted component's props
    /// (the first `props` of them), then the item and index of each client
    /// `each` being painted.
    pub(super) env: Vec<(String, Pv)>,
    pub(super) props: usize,
    /// In a `<template>`'s content, which the browser copies: no first paint.
    pub(super) inert: bool,
    /// A component's `paint`: its groups are marked as in the browser's copy.
    pub(super) paint: bool,
    /// A page, layout or error page: what renders has the request, `cx`,
    /// so an action's form shows what it refused (`Node::Kept`). A
    /// component has none.
    pub(super) has_cx: bool,
    /// A layout with a `<title>`: it writes it when its `TITLE` is true,
    /// which its callers decide at build time (nothing inside has one).
    pub(super) gated: bool,
    /// The names a `---` block's statements bind, borrowed where they are
    /// iterated or matched on.
    pub(super) locals: Vec<String>,
}
