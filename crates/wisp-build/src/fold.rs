//! What a template writes, worked out at build time where it can be: a
//! hole whose value is a literal, and a whole page (with its layouts and
//! components) that reads nothing of the request, which is then baked into
//! the binary with its response head.
//!
//! Only what the runtime would write byte for byte: literals as `Display`
//! writes them (strings, integers, `true`, `false`), escaped by the
//! runtime's own `contexts.rs`; a component's props given as literals (or left
//! to literal defaults); `{#if}` on those. Anything else is left to run.

use crate::contexts::runs_script;
use crate::template::{Node, PropDecl, PropValue, Template};
use crate::ty;

/// A value known at build time.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Lit {
    Str(String),
    Int(i128),
    Bool(bool),
}

impl Lit {
    /// What its `Display` writes.
    fn text(&self) -> String {
        match self {
            Lit::Str(s) => s.clone(),
            Lit::Int(n) => n.to_string(),
            Lit::Bool(b) => b.to_string(),
        }
    }
}

/// `src` as a literal: a string (`"…"` with simple escapes, or raw), an
/// integer in decimal (with `_`s and a type suffix), `true` or `false`.
pub fn literal(src: &str) -> Option<Lit> {
    let s = src.trim();
    match s {
        "true" => return Some(Lit::Bool(true)),
        "false" => return Some(Lit::Bool(false)),
        _ => {}
    }
    if let Some(inner) = s.strip_prefix('"').and_then(|r| r.strip_suffix('"')) {
        let mut out = String::with_capacity(inner.len());
        let mut chars = inner.chars();
        while let Some(c) = chars.next() {
            out.push(match c {
                '"' => return None, // `"a" == "b"`: not one literal
                '\\' => match chars.next()? {
                    '\\' => '\\',
                    '"' => '"',
                    '\'' => '\'',
                    'n' => '\n',
                    't' => '\t',
                    'r' => '\r',
                    '0' => '\0',
                    _ => return None,
                },
                c => c,
            });
        }
        return Some(Lit::Str(out));
    }
    if let Some(r) = s.strip_prefix('r') {
        let hashes = r.len() - r.trim_start_matches('#').len();
        let end = format!("\"{}", "#".repeat(hashes));
        let inner = r[hashes..].strip_prefix('"')?.strip_suffix(&end)?;
        return (!inner.contains(&end)).then(|| Lit::Str(inner.to_string()));
    }
    let (minus, digits) = match s.strip_prefix('-') {
        Some(d) => (true, d),
        None => (false, s),
    };
    let end = digits
        .find(|c: char| !c.is_ascii_digit() && c != '_')
        .unwrap_or(digits.len());
    let (number, suffix) = digits.split_at(end);
    // No suffix is an `i32`; one that is no integer type's is no literal.
    let range = ty::int_range(if suffix.is_empty() { "i32" } else { suffix })?;
    if !number.starts_with(|c: char| c.is_ascii_digit()) {
        return None;
    }
    let n: i128 = number.replace('_', "").parse().ok()?;
    let n = if minus { -n } else { n };
    // One that does not fit its type (`300u8`, `-1u32`, past `i32` with no
    // suffix) is left for rustc to refuse.
    range.contains(&n).then_some(Lit::Int(n))
}

/// `s` escaped for text and quoted attributes: the runtime's `escape`.
pub fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    crate::contexts::escape(&mut out, s);
    out
}

/// `src` when it is `if COND { A } else { B }` of two literals: `COND`, and
/// what each writes, escaped. A condition with a literal or a block in it
/// is left to run.
pub fn either(src: &str) -> Option<(&str, String, String)> {
    let rest = src.trim().strip_prefix("if ")?;
    let open = rest.find('{')?;
    let cond = rest[..open].trim();
    if cond.is_empty() || cond.contains(['"', '\'', '}']) {
        return None;
    }
    let close = open + rest[open..].find('}')?;
    let yes = literal(&rest[open + 1..close])?;
    let other = rest[close + 1..]
        .trim_start()
        .strip_prefix("else")?
        .trim_start();
    let no = literal(other.strip_prefix('{')?.strip_suffix('}')?)?;
    Some((cond, escape(&yes.text()), escape(&no.text())))
}

/// Names bound to values known at build time: a component's props.
type Env = [(String, Lit)];

/// The value of the expression `src`: a literal, or a name in `env`.
fn value(src: &str, env: &Env) -> Option<Lit> {
    let s = src.trim();
    literal(s).or_else(|| env.iter().find(|(n, _)| n == s).map(|(_, v)| v.clone()))
}

/// The condition `src`, when it is a known `bool`, or `!` of one.
fn condition(src: &str, env: &Env) -> Option<bool> {
    match src.trim().strip_prefix('!') {
        Some(rest) => condition(rest, env).map(|b| !b),
        None => match value(src, env)? {
            Lit::Bool(b) => Some(b),
            _ => None,
        },
    }
}

/// What node `n` of `t` writes, when that is known at build time with no
/// names bound: text, and holes of literals.
pub fn fixed(n: &Node, t: &Template) -> Option<String> {
    fixed_in(n, t, &[])
}

fn fixed_in(n: &Node, t: &Template, env: &Env) -> Option<String> {
    Some(match n {
        Node::Text(i) => t.chunks[*i].clone(),
        Node::Expr(c) => escape(&value(&c.src, env)?.text()),
        Node::Html(c) => value(&c.src, env)?.text(),
        Node::Attr { name, code, url } => {
            let v = escape(&value(&code.src, env)?.text());
            if *url && runs_script(&v) {
                return None;
            }
            format!(" {name}=\"{v}\"")
        }
        // `class:x={…}` needs what is before it: see `Fold::nodes`.
        Node::Bool {
            name,
            code,
            class: false,
        } => match condition(&code.src, env)? {
            true => format!(" {name}"),
            false => String::new(),
        },
        _ => return None,
    })
}

/// A page's head and body as a template writes them.
#[derive(Default)]
pub struct Doc {
    pub head: String,
    pub body: String,
}

/// What renders a slot, into the document. What goes in one writes where
/// it was written for (the page to the body, a component's children where
/// it is used), wherever the slot is.
type Slot<'s> = &'s dyn Fn(&mut Doc) -> Option<()>;

/// A component's template and props, by name, when it has no browser code.
type Comp<'a> = &'a dyn Fn(&str) -> Option<(&'a Template, &'a [PropDecl])>;

/// Runs templates at build time.
pub struct Fold<'a> {
    pub comp: Comp<'a>,
    /// How many components deep the walk is: one that renders itself is not baked.
    pub depth: std::cell::Cell<u8>,
}

impl Fold<'_> {
    /// What `layers` write, the outermost layout first and the page last,
    /// each rendering the next in its slot: `None` if any of it is not
    /// known at build time.
    pub fn page(&self, layers: &[&Template]) -> Option<Doc> {
        let mut doc = Doc::default();
        self.layers(layers, &mut doc)?;
        Some(doc)
    }

    fn layers(&self, layers: &[&Template], doc: &mut Doc) -> Option<()> {
        let (t, inner) = layers.split_first()?;
        if t.is_live() {
            return None;
        }
        let slot = |doc: &mut Doc| match inner.is_empty() {
            true => Some(()),
            false => self.layers(inner, doc),
        };
        // The innermost `<title>` is the one written: an outer one is left out.
        if t.has_title() && inner.iter().any(|t| t.has_title()) {
            let nodes: Vec<Node> = (t.nodes.iter())
                .filter(|n| !t.is_title(n))
                .map(|n| match n {
                    Node::Head(b) => {
                        Node::Head(b.iter().filter(|n| !t.is_title(n)).cloned().collect())
                    }
                    n => n.clone(),
                })
                .collect();
            return self.nodes(&nodes, t, &[], doc, false, &slot);
        }
        self.nodes(&t.nodes, t, &[], doc, false, &slot)
    }

    fn nodes(
        &self,
        nodes: &[Node],
        t: &Template,
        env: &Env,
        doc: &mut Doc,
        head: bool,
        slot: Slot,
    ) -> Option<()> {
        // Where the value of a URL attribute with holes began.
        let mut url = 0;
        for n in nodes {
            let out = if head { &mut doc.head } else { &mut doc.body };
            match n {
                Node::Head(body) => self.nodes(body, t, env, doc, true, slot)?,
                Node::Render => slot(doc)?,
                Node::UrlStart { prefix } => url = out.len().saturating_sub(prefix.len()),
                Node::UrlEnd if runs_script(&out[url..]) => return None,
                Node::UrlEnd => {}
                // What an action refused, which a GET (all that is baked)
                // never has: an input's value sent again, a select's choice
                // by it, and its problem. The input's own value is written.
                Node::Kept { own, .. } => {
                    if let Some(own) = own {
                        self.nodes(own, t, env, doc, head, slot)?;
                    }
                }
                Node::Chosen { own: None, .. }
                | Node::Problem { .. }
                | Node::Invalid { .. }
                | Node::Selected(_) => {}
                Node::Bool {
                    name,
                    code,
                    class: true,
                } => {
                    if condition(&code.src, env)? {
                        if !out.ends_with(['"', '\'']) {
                            out.push(' ');
                        }
                        out.push_str(name);
                    }
                }
                Node::If {
                    branches,
                    otherwise,
                } => {
                    let mut taken = None;
                    for (cond, body) in branches {
                        if condition(&cond.src, env)? {
                            taken = Some(body);
                            break;
                        }
                    }
                    if let Some(body) = taken.or(otherwise.as_ref()) {
                        self.nodes(body, t, env, doc, head, slot)?;
                    }
                }
                Node::Component {
                    name,
                    props,
                    children,
                    ..
                } => {
                    // `client:visible` and the like are the browser's.
                    if props.iter().any(|p| p.name.starts_with("client:")) {
                        return None;
                    }
                    let (ct, decls) = (self.comp)(name)?;
                    let mut own = Vec::new();
                    for d in decls {
                        let given = props.iter().find(|p| p.name == d.name);
                        let v = match given.map(|p| &p.value) {
                            Some(PropValue::Text(s)) => Some(Lit::Str(s.clone())),
                            Some(PropValue::Flag) => Some(Lit::Bool(true)),
                            Some(PropValue::Expr(c)) => value(&c.src, env),
                            None => d.default.as_deref().and_then(literal),
                            Some(_) => None,
                        };
                        // A prop not known here fails only a hole that reads it.
                        if let Some(v) = v.filter(|v| fits(v, &d.ty)) {
                            own.push((d.name.clone(), v));
                        }
                    }
                    let kids = |doc: &mut Doc| match children {
                        Some(c) => self.nodes(c, t, env, doc, head, slot),
                        None => Some(()),
                    };
                    if self.depth.get() >= 64 {
                        return None;
                    }
                    self.depth.set(self.depth.get() + 1);
                    let done = self.nodes(&ct.nodes, ct, &own, doc, head, &kids);
                    self.depth.set(self.depth.get() - 1);
                    done?;
                }
                n => out.push_str(&fixed_in(n, t, env)?),
            }
        }
        Some(())
    }
}

/// Whether a prop of type `ty` given `v` writes what `v` does: text, an
/// integer, a `bool`, or any of them as an `impl Display`.
fn fits(v: &Lit, ty: &str) -> bool {
    let ty = ty.trim();
    (ty.starts_with("impl ") && ty.ends_with("Display"))
        || match v {
            Lit::Str(_) => ty::is_text(ty),
            Lit::Int(n) => {
                ty::int_range(ty::last_segment(ty::unref(ty).trim())).is_some_and(|r| r.contains(n))
            }
            Lit::Bool(_) => ty == "bool",
        }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::template::parse;

    #[test]
    fn literals() {
        for (src, want) in [
            ("\"a<b\"", Some(Lit::Str("a<b".into()))),
            (r#""say \"hi\"\n""#, Some(Lit::Str("say \"hi\"\n".into()))),
            ("r#\"a\"b\"#", Some(Lit::Str("a\"b".into()))),
            ("r\"x\"", Some(Lit::Str("x".into()))),
            (" 42 ", Some(Lit::Int(42))),
            ("-7i64", Some(Lit::Int(-7))),
            ("1_000u32", Some(Lit::Int(1000))),
            ("007", Some(Lit::Int(7))),
            ("255u8", Some(Lit::Int(255))),
            ("-2147483648", Some(Lit::Int(-2147483648))),
            ("true", Some(Lit::Bool(true))),
            ("false", Some(Lit::Bool(false))),
        ] {
            assert_eq!(literal(src), want, "{src}");
        }
        for src in [
            "\"a\" == \"b\"",
            "\"\\u{41}\"",
            "1.5",
            "1e3",
            "0x10",
            "-x",
            "x",
            "row",
            "b\"x\"",
            "'c'",
            "4u7",
            "340282366920938463463374607431768211456",
            // Not of their type: rustc says so, not the page.
            "300u8",
            "-1u32",
            "2147483648",
            "-129i8",
        ] {
            assert_eq!(literal(src), None, "{src}");
        }
        assert_eq!(
            escape("<a href=\"x\">'&'</a>"),
            "&lt;a href=&quot;x&quot;&gt;&#39;&amp;&#39;&lt;/a&gt;"
        );
    }

    #[test]
    fn either_of_two_literals() {
        let two = |c: &'static str, a: &str, b: &str| Some((c, a.to_string(), b.to_string()));
        for (src, want) in [
            (
                "if p.active { \"on\" } else { \"off\" }",
                two("p.active", "on", "off"),
            ),
            ("if !x {\"a<\"} else {1}", two("!x", "a&lt;", "1")),
            (
                "if let Some(_) = y { \"a\" } else { \"\" }",
                two("let Some(_) = y", "a", ""),
            ),
        ] {
            assert_eq!(either(src), want, "{src}");
        }
        for src in [
            "if x { \"a\" } else if y { \"b\" } else { \"c\" }",
            "if x { a } else { \"b\" }",
            "if x { \"a}\" } else { \"b\" }",
            "if s == \"x\" { \"a\" } else { \"b\" }",
            "if x { \"a\" } else { \"b\" }.len()",
            "if { \"a\" } else { \"b\" }",
            "x",
        ] {
            assert_eq!(either(src), None, "{src}");
        }
    }

    /// The runs a release build writes out: text and literal holes, with
    /// anything else breaking one.
    #[test]
    fn fixed_nodes() {
        let t = parse(
            "<p title={\"a&b\"} hidden={false} open={true}>{\"<x>\"}{7}{@html \"<i>\"}{n}</p><a href={\"/p\"}>x</a><a href={\"javascript:x\"}>y</a>",
        )
        .unwrap();
        let out: Vec<Option<String>> = t.nodes.iter().map(|n| fixed(n, &t)).collect();
        let joined: String = out.iter().map(|o| o.as_deref().unwrap_or("|")).collect();
        assert_eq!(
            joined,
            "<p title=\"a&amp;b\" open>&lt;x&gt;7<i>|</p><a href=\"/p\">x</a><a|>y</a>"
        );
    }

    fn page(layers: &[&str], comps: &[(&str, &str)]) -> Option<Doc> {
        let comps: Vec<(String, Template)> = comps
            .iter()
            .map(|(name, src)| (name.to_string(), parse(src).unwrap()))
            .collect();
        let decls: Vec<Vec<PropDecl>> = comps
            .iter()
            .map(|(_, t)| t.props.as_ref().map_or(Vec::new(), |(p, _)| p.clone()))
            .collect();
        let comp = |name: &str| {
            let k = comps.iter().position(|(n, _)| n == name)?;
            (!comps[k].1.is_live()).then(|| (&comps[k].1, &decls[k][..]))
        };
        let ts: Vec<Template> = layers.iter().map(|s| parse(s).unwrap()).collect();
        let refs: Vec<&Template> = ts.iter().collect();
        Fold {
            comp: &comp,
            depth: Default::default(),
        }
        .page(&refs)
    }

    #[test]
    fn one_title_the_innermost() {
        let layout =
            "<head><title>L</title><meta name=\"a\"></head><main>{@render children()}</main>";
        let doc = page(&[layout, "<title>P</title><p>x</p>"], &[]).unwrap();
        assert_eq!(doc.head, "<meta name=\"a\"><title>P</title>");
        let doc = page(
            &[layout, "<title>Q</title>{@render children()}", "<p>x</p>"],
            &[],
        )
        .unwrap();
        assert_eq!(doc.head, "<meta name=\"a\"><title>Q</title>");
        let doc = page(&[layout, "<p>x</p>"], &[]).unwrap();
        assert_eq!(doc.head, "<title>L</title><meta name=\"a\">");
    }

    #[test]
    fn pages_known_at_build_time() {
        let card = (
            "Card",
            "{@props title: &str, count: u32 = 0, featured: bool = false, note: impl std::fmt::Display = 1}\n<h2 class:hot={featured}>{title}{#if featured} ★{:else} ☆{/if}</h2><p>{count} {note}</p>{@render children()}",
        );
        let doc = page(
            &[
                "<nav>{\"Tom & Jerry\"}</nav>{@render children()}<footer/>",
                "<wisp:head><title>T</title></wisp:head><main>{@render children()}</main>",
                "<Card title=\"Hi\" count={3} featured><b>{1}</b></Card><Card title=\"Lo\" />",
            ],
            &[card],
        )
        .unwrap();
        assert_eq!(doc.head, "<title>T</title>");
        assert_eq!(
            doc.body,
            "<nav>Tom &amp; Jerry</nav><main><h2 class=\"hot\">Hi ★</h2><p>3 1</p><b>1</b>\
             <h2 class=\"\">Lo ☆</h2><p>0 1</p></main><footer/>"
        );

        // Anything read at run time, or browser code, keeps the page from
        // being baked.
        for src in [
            "{n}",
            "<Card title={t} />",
            "<Card title=\"x\" featured={on} />",
            "{#each xs as x}{x}{/each}",
            "{@const a = 1}",
            "<a href=\"{\"javascript:x\"}\">x</a>",
            "<button on:click=\"n++\">x</button>",
            "<Live />",
        ] {
            let live = ("Live", "<p>{:n}</p><script>let n = 1</script>");
            assert!(page(&[src], &[card, live]).is_none(), "{src}");
        }
        assert!(
            page(
                &[
                    "<p>{:n}</p><script>let n = 1</script>{@render children()}",
                    "x"
                ],
                &[]
            )
            .is_none()
        );

        // An action's form: a GET never has what it refused, so its inputs
        // show their own values, and their problems nothing.
        let form = "<form method=\"post\"><input name=\"a\"><input name=\"b\" value={\"v\"}>\
                    <textarea name=\"t\">hi</textarea><select name=\"s\"><option value=\"x\">X</option></select>\
                    {cx.problem(\"a\")}</form>";
        assert_eq!(
            page(&[form], &[]).unwrap().body,
            "<form method=\"post\"><input name=\"a\"><input name=\"b\" value=\"v\"><textarea name=\"t\">hi</textarea>\
             <select name=\"s\"><option value=\"x\">X</option></select></form>"
        );
        // A select chosen by a value of its own is read at run time.
        assert!(
            page(
                &["<form method=\"post\"><select name=\"s\" value={\"x\"}></select></form>"],
                &[]
            )
            .is_none()
        );

        // The page writes to the body wherever its layout's slot is.
        let doc = page(
            &["<wisp:head>{@render children()}</wisp:head>", "<p>x</p>"],
            &[],
        )
        .unwrap();
        assert_eq!((doc.head.as_str(), doc.body.as_str()), ("", "<p>x</p>"));
    }
}
