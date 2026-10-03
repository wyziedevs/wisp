//! Accessibility lints, found as the template parser reads each tag. They
//! are warnings: the build goes on. `<!-- wisp-ignore a11y-img-alt -->` on
//! the line before an element silences that one there.

use crate::template::{Dir, Directive};

/// A lint at a line of its file: its name (`img-alt`) and what to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Lint {
    pub line: u32,
    pub name: &'static str,
    pub msg: String,
}

/// What the parser tells as it goes, and the lints so far.
#[derive(Default)]
pub struct Checker {
    pub lints: Vec<Lint>,
    /// `wisp-ignore` comments: the line each ends on, and its words.
    ignores: Vec<(u32, String)>,
    /// The `<button>`s, `<a>`s and `<label>`s open: tag, line, whether it has what
    /// it needs (a name; a control) yet.
    open: Vec<(&'static str, u32, bool)>,
    /// The last heading's level.
    heading: Option<u8>,
}

/// Tags a click already works on, by keyboard too.
const INTERACTIVE: [&str; 9] = [
    "a", "button", "input", "select", "textarea", "summary", "details", "option", "label",
];

/// What a `<label>` labels.
const CONTROLS: [&str; 7] = [
    "input", "select", "textarea", "button", "meter", "output", "progress",
];

/// WAI-ARIA 1.2's attributes, and 1.3's that browsers have.
const ARIA: [&str; 53] = [
    "activedescendant",
    "atomic",
    "autocomplete",
    "braillelabel",
    "brailleroledescription",
    "busy",
    "checked",
    "colcount",
    "colindex",
    "colindextext",
    "colspan",
    "controls",
    "current",
    "describedby",
    "description",
    "details",
    "disabled",
    "dropeffect",
    "errormessage",
    "expanded",
    "flowto",
    "grabbed",
    "haspopup",
    "hidden",
    "invalid",
    "keyshortcuts",
    "label",
    "labelledby",
    "level",
    "live",
    "modal",
    "multiline",
    "multiselectable",
    "orientation",
    "owns",
    "placeholder",
    "posinset",
    "pressed",
    "readonly",
    "relevant",
    "required",
    "roledescription",
    "rowcount",
    "rowindex",
    "rowindextext",
    "rowspan",
    "selected",
    "setsize",
    "sort",
    "valuemax",
    "valuemin",
    "valuenow",
    "valuetext",
];

impl Checker {
    /// An HTML comment ending on `line`.
    pub fn comment(&mut self, body: &str, line: u32) {
        if let Some(words) = body.trim().strip_prefix("wisp-ignore") {
            self.ignores.push((line, words.to_string()));
        }
    }

    /// Text, a hole or a component inside the elements open: a name for
    /// the buttons; a component may hold a control, too.
    pub fn content(&mut self, component: bool) {
        for (tag, _, ok) in &mut self.open {
            *ok |= *tag != "label" || component;
        }
    }

    /// The start tag of `tag` (lowercase) on `line`, its attributes (with
    /// their plain values) and its browser directives, read whole.
    pub fn element(
        &mut self,
        tag: &str,
        line: u32,
        attrs: &[(String, Option<String>)],
        dirs: &[Directive],
    ) {
        let value = |n: &str| attrs.iter().find(|a| a.0 == n).map(|a| a.1.as_deref());
        let live = |n: &str| dirs.iter().any(|d| d.kind == Dir::Attr && d.name == n);
        let spread = dirs.iter().any(|d| d.kind == Dir::Spread);
        let has = |n: &str| value(n).is_some() || live(n) || spread;
        // A name for a button around it.
        let named = has("aria-label") || has("aria-labelledby") || has("title");
        let alt = value("alt").flatten().is_some_and(|a| !a.trim().is_empty()) || live("alt");
        if named || (tag == "img" && alt) {
            self.content(false);
        }
        if CONTROLS.contains(&tag) {
            for (t, _, ok) in &mut self.open {
                *ok |= *t == "label";
            }
        }
        match tag {
            "img" if !has("alt") => self.lint(
                line,
                "img-alt",
                "<img> has no alt: say what it shows, or alt=\"\" if it shows nothing that matters",
            ),
            "a" if !has("href") => self.lint(
                line,
                "anchor-href",
                "<a> has no href: a link goes somewhere; for an action, use a <button>",
            ),
            "a" if value("href") == Some(Some("#")) => self.lint(
                line,
                "anchor-href",
                "<a href=\"#\"> goes nowhere: link to a place, or use a <button>",
            ),
            "a" => self.open.push(("a", line, named)),
            "button" => self.open.push(("button", line, named)),
            "input" | "select" | "textarea"
                if !named
                    && !has("id")
                    && !self.open.iter().any(|o| o.0 == "label")
                    && !matches!(
                        value("type"),
                        Some(Some("hidden" | "submit" | "button" | "reset" | "image"))
                    ) =>
            {
                self.lint(line, "input-label", &format!("<{tag}> has no label: wrap it in a <label>, or give it an id for a <label for>, or an aria-label (a placeholder is not a name)"));
            }
            "label" => self.open.push(("label", line, has("for"))),
            _ => {}
        }
        if let Some(level) = (tag.strip_prefix('h').and_then(|n| n.parse::<u8>().ok()))
            .filter(|n| (1..=6).contains(n))
        {
            if let Some(last) = self.heading.filter(|&l| level > l + 1) {
                self.lint(line, "heading-order", &format!("<h{level}> after <h{last}> skips a level: screen readers list headings as an outline"));
            }
            self.heading = Some(level);
        }
        let clicks = dirs.iter().any(|d| {
            d.kind == Dir::On
                && d.name == "click"
                && !d
                    .mods
                    .iter()
                    .any(|m| matches!(m.as_str(), "window" | "document" | "outside"))
        }) || value("onclick").is_some();
        let keys = dirs.iter().any(|d| {
            d.kind == Dir::On && matches!(d.name.as_str(), "keydown" | "keyup" | "keypress")
        }) || ["onkeydown", "onkeyup", "onkeypress"]
            .iter()
            .any(|k| value(k).is_some());
        // A custom element (`<sl-button>`) keeps its keyboard inside it.
        let custom = tag.contains('-');
        if clicks && !custom && !INTERACTIVE.contains(&tag) && !(has("role") && keys) {
            self.lint(line, "click-events", &format!("<{tag}> takes clicks but not the keyboard: use a <button>, or give it a role and on:keydown"));
        }
        if value("autofocus").is_some() {
            self.lint(line, "autofocus", "autofocus moves screen readers and keyboards past what comes before it; leave the focus where the page starts");
        }
        if let Some(Some(n)) = value("tabindex")
            && n.trim().parse::<i64>().is_ok_and(|n| n > 0)
        {
            self.lint(line, "tabindex", "a tabindex above 0 jumps the tab order; use 0, or put the element where it should come");
        }
        let names = (attrs.iter().map(|a| a.0.as_str())).chain(
            dirs.iter()
                .filter(|d| d.kind == Dir::Attr)
                .map(|d| d.name.as_str()),
        );
        for n in names {
            if let Some(a) = n.strip_prefix("aria-")
                && !ARIA.contains(&a)
            {
                self.lint(
                    line,
                    "aria-attr",
                    &format!("`{n}` is not an ARIA attribute"),
                );
            }
        }
    }

    /// The end tag of `tag`.
    pub fn end(&mut self, tag: &str) {
        let Some(k) = self.open.iter().rposition(|o| o.0 == tag) else {
            return;
        };
        let (_, line, ok) = self.open.remove(k);
        if ok {
            return;
        }
        if tag == "button" {
            self.lint(
                line,
                "button-name",
                "<button> has no text: screen readers say only \"button\"; add text or aria-label",
            );
        } else if tag == "a" {
            self.lint(
                line,
                "link-name",
                "<a> has no text: screen readers say only \"link\"; add text, an <img alt>, or aria-label",
            );
        } else {
            self.lint(line, "label-control", "<label> labels nothing: put its control inside it, or give it for=\"the control's id\"");
        }
    }

    fn lint(&mut self, line: u32, name: &'static str, msg: &str) {
        let word = format!("a11y-{name}");
        let quiet = (self.ignores.iter())
            .any(|(l, words)| *l + 1 == line && words.split_whitespace().any(|w| w == word));
        if !quiet {
            self.lints.push(Lint {
                line,
                name,
                msg: msg.to_string(),
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::template::parse;

    /// The lints of `src`, as `line name`.
    fn lints(src: &str) -> Vec<String> {
        let t = parse(src).unwrap();
        t.lints
            .iter()
            .map(|l| format!("{} {}", l.line, l.name))
            .collect()
    }

    #[test]
    fn each_lint() {
        assert_eq!(
            lints("<img src=\"a.png\">\n<img alt=\"\"><img {alt}>"),
            ["1 img-alt"]
        );
        assert_eq!(
            lints("<a>x</a><a href=\"#\">y</a><a {href}>z</a><a href={u}>w</a>"),
            ["1 anchor-href", "1 anchor-href"]
        );
        assert_eq!(
            lints(
                "<input id=\"a\" autofocus>\n<p tabindex=\"2\" aria-lable=\"x\" aria-label=\"y\">"
            ),
            ["1 autofocus", "2 tabindex", "2 aria-attr"]
        );
        assert!(
            lints("<p tabindex=\"0\" :aria-hidden=\"h\">x</p><script>let h</script>").is_empty()
        );
        assert_eq!(
            lints("<h1>a</h1>\n<h3>b</h3>\n<h2>c</h2><h3>d</h3>"),
            ["2 heading-order"]
        );
        assert_eq!(
            lints(
                "<button></button>\n<button><svg></svg></button><button>{n}</button>\n\
                 <button aria-label=\"Close\"><svg/></button><button><img alt=\"Go\"></button>"
            ),
            ["1 button-name", "2 button-name"]
        );
        assert_eq!(
            lints(
                "<a href=\"/\"></a>
<a href=\"/\"><svg/></a><a href=\"/\">Home</a>
                 <a href=\"/\" aria-label=\"Home\"><svg/></a><a href=\"/\">{n}</a><a href=\"/\"><img alt=\"x\"></a>"
            ),
            ["1 link-name", "2 link-name"]
        );
        assert_eq!(
            lints(
                "<input name=\"a\">
<label>N <input></label><input id=\"b\"><input type=\"hidden\">
                 <textarea></textarea><select aria-label=\"S\"></select><input type=\"submit\"><input {id}>"
            ),
            ["1 input-label", "3 input-label"]
        );
        assert_eq!(
            lints(
                "<label>Name</label>\n<label>Name <input></label><label for=\"n\">N</label>\
                 <label><Field /></label>"
            ),
            ["1 label-control"]
        );
    }

    #[test]
    fn clicks_need_the_keyboard_too() {
        let js = "<script>function go() {}</script>";
        assert_eq!(
            lints(&format!(
                "<div on:click=\"go\">x</div>\n<li role=\"button\" on:click=\"go\">y</li>{js}"
            )),
            ["1 click-events", "2 click-events"]
        );
        assert!(
            lints(&format!(
                "<li role=\"button\" on:click=\"go\" on:keydown=\"go\">y</li>\
                 <button on:click=\"go\">b</button><div on:click.outside=\"go\">z</div>\
                 <sl-button on:click=\"go\">c</sl-button>{js}"
            ))
            .is_empty()
        );
    }

    #[test]
    fn a_comment_on_the_line_before_silences_one() {
        let src = "<!-- wisp-ignore a11y-img-alt -->\n<img src=\"a\"><a>x</a>\n\
                   <!-- wisp-ignore a11y-autofocus a11y-tabindex -->\n\n<input id=\"a\" autofocus>";
        assert_eq!(lints(src), ["2 anchor-href", "5 autofocus"]);
    }
}
