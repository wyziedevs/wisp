//! The component workshop at `/_wisp/components`, in debug builds only:
//! every component, each story rendered by the server in a frame of its
//! own (the app's shell and CSS, none of this page's), and controls for its
//! simple props that render it again. Stories are `Name.stories.wisp`
//! files beside the components (see `wisp-build`'s `stories.rs`).

use crate::html::text;
use crate::rt::{Control, Shelf, Story};
use crate::{App, Cx, Out};
use std::fmt::Write as _;

pub(crate) enum Answer {
    /// A page of the workshop itself.
    Page(String),
    /// A story, rendered into `out` as a page is.
    Frame,
    Missing,
}

/// `path` is under `/_wisp/components`.
pub(crate) fn answer<A: App>(cx: &Cx, out: &mut Out) -> Answer {
    let rest = cx.path().trim_start_matches("/_wisp/components");
    let parts: Vec<&str> = rest.split('/').filter(|s| !s.is_empty()).collect();
    let shelves = A::workshop();
    let find = |name: &str| shelves.iter().find(|s| s.name == name);
    let story = |s: &'static Shelf, slug: &str| s.stories.iter().find(|x| x.slug == slug);
    match parts[..] {
        [] if rest.len() <= 1 => Answer::Page(index(shelves)),
        [name] => match find(name) {
            Some(s) => Answer::Page(page(shelves, s, s.stories.first(), cx)),
            None => Answer::Missing,
        },
        [name, slug] => match find(name).and_then(|s| Some((s, story(s, slug)?))) {
            Some((s, st)) => Answer::Page(page(shelves, s, Some(st), cx)),
            None => Answer::Missing,
        },
        [name, slug, "frame"] => match find(name).and_then(|s| story(s, slug)) {
            Some(st) => {
                out.clear();
                (st.render)(out, cx);
                Answer::Frame
            }
            None => Answer::Missing,
        },
        _ => Answer::Missing,
    }
}

/// Every component, and what it takes.
fn index(shelves: &[Shelf]) -> String {
    let mut main = String::from("<header><h1>Components</h1><p class=\"sub\">");
    let _ = write!(
        main,
        "{} in <code>src/components</code>. A <code>Name.stories.wisp</code> beside one holds its stories: \
         <code>{{#story \"Featured\"}}&lt;Card featured title=\"x\" /&gt;{{/story}}</code>.</p></header>",
        match shelves.len() {
            1 => "One component".to_string(),
            n => format!("{n} components"),
        }
    );
    if shelves.is_empty() {
        main.push_str(
            "<p class=\"empty\">No components yet: add <code>src/components/Card.wisp</code>.</p>",
        );
    }
    main.push_str("<ul class=\"cards\">");
    for s in shelves {
        let _ = write!(main, "<li><a href=\"/_wisp/components/{}\"><b>", s.name);
        text(&mut main, s.name);
        main.push_str("</b><code>");
        text(&mut main, s.file);
        main.push_str("</code><span>");
        let props: Vec<&str> = s.props.iter().map(|p| p.name).collect();
        match props.is_empty() {
            true => main.push_str("No props"),
            false => text(&mut main, &props.join(", ")),
        }
        main.push_str("</span><small>");
        match s.stories.len() {
            0 => main.push_str("No story"),
            1 => main.push_str("1 story"),
            n => {
                let _ = write!(main, "{n} stories");
            }
        }
        main.push_str("</small></a></li>");
    }
    main.push_str("</ul>");
    chrome("Components", shelves, None, &main)
}

/// A component's story, or why it has none.
fn page(shelves: &[Shelf], s: &Shelf, story: Option<&Story>, cx: &Cx) -> String {
    let mut main = String::from(
        "<header><p class=\"crumb\"><a href=\"/_wisp/components\">Components</a> / <code>",
    );
    text(&mut main, s.file);
    main.push_str("</code></p><div class=\"title\"><h1>");
    text(&mut main, s.name);
    if let Some(st) = story {
        main.push_str(" <span>");
        text(&mut main, st.name);
        main.push_str("</span>");
    }
    main.push_str("</h1><button class=\"wisp-button wisp-small\" type=\"button\" data-open=\"");
    let at = match story {
        Some(st) => format!("{}\n{}", st.file, st.line),
        None => format!("{}\n1", s.file),
    };
    text(&mut main, &at);
    main.push_str("\">Open in editor</button></div></header>");
    let Some(st) = story else {
        main.push_str("<p class=\"empty\">");
        text(&mut main, s.note);
        main.push_str("</p>");
        return chrome(s.name, shelves, Some((s, None)), &main);
    };
    main.push_str("<div class=\"split\"><section class=\"stage\"><iframe title=\"");
    text(&mut main, &format!("{}: {}", s.name, st.name));
    let _ = write!(
        main,
        "\" src=\"/_wisp/components/{}/{}/frame?",
        s.name, st.slug
    );
    text(&mut main, cx.query_string());
    main.push_str(
        "\"></iframe></section><section class=\"props\"><h2>Props</h2><form id=\"props\">",
    );
    for p in s.props {
        let given = cx.query(p.name);
        let value = given.as_deref().or_else(|| {
            st.values
                .iter()
                .find(|(k, _)| *k == p.name)
                .map(|(_, v)| *v)
        });
        main.push_str("<label><span>");
        text(&mut main, p.name);
        main.push_str("<small>");
        text(&mut main, p.ty);
        main.push_str("</small></span>");
        let (kind, rest) = match p.control {
            Some(Control::Text) => ("text", ""),
            Some(Control::Number) => ("number", " step=\"any\""),
            Some(Control::Check) => ("checkbox", ""),
            None => {
                main.push_str("<em>As the story has it</em></label>");
                continue;
            }
        };
        let _ = write!(main, "<input type=\"{kind}\" name=\"");
        text(&mut main, p.name);
        main.push('"');
        main.push_str(rest);
        if given.is_some() {
            main.push_str(" data-set");
        }
        match (p.control, value) {
            (Some(Control::Check), Some("true")) => main.push_str(" checked"),
            (Some(Control::Check), _) => {}
            (_, Some(v)) => {
                main.push_str(" value=\"");
                text(&mut main, v);
                main.push('"');
            }
            (_, None) => main.push_str(" placeholder=\"As the story has it\""),
        }
        main.push_str("></label>");
    }
    if s.props.is_empty() {
        main.push_str("<p class=\"empty\">No props.</p>");
    }
    main.push_str("</form><button class=\"wisp-button wisp-ghost wisp-small\" type=\"button\" id=\"reset\">Reset</button></section></div>");
    chrome(s.name, shelves, Some((s, Some(st))), &main)
}

/// The page around `main`: Wisp's own styles, and the list of components
/// and stories, the one shown marked.
fn chrome(
    title: &str,
    shelves: &[Shelf],
    on: Option<(&Shelf, Option<&Story>)>,
    main: &str,
) -> String {
    let mut h = String::with_capacity(16 * 1024);
    h.push_str("<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width, initial-scale=1\"><title>");
    text(&mut h, title);
    h.push_str(" · Wisp</title><style>");
    h.push_str(crate::http::UI_CSS);
    h.push_str(CSS);
    h.push_str("</style></head><body><nav><a class=\"brand\" href=\"/_wisp/components\">Components</a><ul>");
    for s in shelves {
        let here = on.is_some_and(|(o, _)| std::ptr::eq(o, s));
        let _ = write!(h, "<li><a href=\"/_wisp/components/{}\"", s.name);
        if here {
            h.push_str(" class=\"on\"");
        }
        h.push('>');
        text(&mut h, s.name);
        h.push_str("</a>");
        if here && s.stories.len() > 1 {
            h.push_str("<ul>");
            for st in s.stories {
                let _ = write!(
                    h,
                    "<li><a href=\"/_wisp/components/{}/{}\"",
                    s.name, st.slug
                );
                if on.is_some_and(|(_, x)| x.is_some_and(|x| std::ptr::eq(x, st))) {
                    h.push_str(" aria-current=\"page\"");
                }
                h.push('>');
                text(&mut h, st.name);
                h.push_str("</a></li>");
            }
            h.push_str("</ul>");
        }
        h.push_str("</li>");
    }
    h.push_str("</ul></nav><main>");
    h.push_str(main);
    h.push_str("</main><script>");
    h.push_str(JS);
    h.push_str("</script></body></html>");
    h
}

const CSS: &str = r#"
body { display: grid; grid-template-columns: 14rem 1fr; min-height: 100vh; margin: 0; background: radial-gradient(56rem 28rem at 50% -6rem, var(--wisp-glow), transparent 70%) no-repeat, var(--wisp-paper); color: var(--wisp-ink); font: 400 0.875rem/1.25rem var(--wisp-sans); }
nav { padding: 1.25rem 0.75rem; border-right: 1px solid var(--wisp-line); background: var(--wisp-panel); }
nav ul { margin: 0; padding: 0; list-style: none; }
nav ul ul { margin: 0.125rem 0 0.25rem 0.75rem; padding-left: 0.5rem; border-left: 1px solid var(--wisp-line); }
nav a { display: block; padding: 0.375rem 0.625rem; border-radius: var(--wisp-radius); color: var(--wisp-slate); text-decoration: none; }
nav a:hover { background: var(--wisp-inset); color: var(--wisp-ink); }
nav a.on, nav a[aria-current] { color: var(--wisp-ink); font-weight: 600; }
nav a[aria-current] { background: var(--wisp-inset); box-shadow: inset 2px 0 var(--wisp-accent); }
nav .brand { margin-bottom: 0.75rem; color: var(--wisp-ink); font-weight: 600; letter-spacing: -0.01em; }
a:focus-visible, input:focus-visible { outline: 2px solid var(--wisp-accent); outline-offset: 1px; }
main { box-sizing: border-box; min-width: 0; padding: 1.5rem 2rem 2rem; }
h1 { margin: 0; font-size: 1.5rem; line-height: 2rem; font-weight: 600; letter-spacing: -0.025em; }
h1 span { color: var(--wisp-slate); font-weight: 400; }
h2 { margin: 0 0 0.75rem; color: var(--wisp-slate); font-size: 0.75rem; line-height: 1rem; font-weight: 600; letter-spacing: 0.05em; text-transform: uppercase; }
header { margin-bottom: 1.25rem; }
.title { display: flex; align-items: center; justify-content: space-between; gap: 1rem; }
.crumb { margin: 0 0 0.25rem; color: var(--wisp-ash); }
.crumb a { color: var(--wisp-accent); text-decoration: none; }
.sub { margin: 0.25rem 0 0; color: var(--wisp-slate); }
code { font: 400 0.8125rem/1.25rem var(--wisp-mono); }
.empty { color: var(--wisp-slate); }
.cards { display: grid; grid-template-columns: repeat(auto-fill, minmax(15rem, 1fr)); gap: 0.75rem; margin: 0; padding: 0; list-style: none; }
.cards a { display: grid; gap: 0.25rem; padding: 1rem; border: 1px solid var(--wisp-line); border-radius: var(--wisp-radius); background: var(--wisp-panel); box-shadow: var(--wisp-raised); color: inherit; text-decoration: none; transition: border-color 250ms var(--wisp-change); }
.cards a:hover { border-color: var(--wisp-accent-edge); }
.cards b { font-size: 1rem; font-weight: 600; }
.cards code, .cards small { color: var(--wisp-ash); }
.cards span { color: var(--wisp-slate); overflow-wrap: anywhere; }
.split { display: grid; grid-template-columns: 1fr 18rem; gap: 1rem; align-items: start; }
.stage { border: 1px solid var(--wisp-line); border-radius: var(--wisp-radius); background: var(--wisp-panel); box-shadow: var(--wisp-raised); overflow: hidden; }
.stage iframe { display: block; width: 100%; height: 70vh; border: 0; resize: vertical; background: var(--wisp-paper); }
.props { padding: 1rem; border: 1px solid var(--wisp-line); border-radius: var(--wisp-radius); background: var(--wisp-panel); box-shadow: var(--wisp-raised); }
.props label { display: grid; gap: 0.25rem; margin: 0 0 0.75rem; }
.props label span { display: flex; align-items: baseline; justify-content: space-between; gap: 0.5rem; font: 500 0.8125rem/1.25rem var(--wisp-mono); }
.props small { color: var(--wisp-ash); font-weight: 400; overflow-wrap: anywhere; }
.props em { color: var(--wisp-ash); font-style: normal; }
.props input:not([type=checkbox]) { box-sizing: border-box; width: 100%; padding: 0.375rem 0.625rem; border: 1px solid var(--wisp-line); border-radius: var(--wisp-radius); background: var(--wisp-inset); color: var(--wisp-ink); font: 400 0.8125rem/1.25rem var(--wisp-mono); }
.props input[type=checkbox] { justify-self: start; width: 1rem; height: 1rem; margin: 0; accent-color: var(--wisp-accent); }
@media (max-width: 52rem) { body { grid-template-columns: 1fr; } nav { border-right: 0; border-bottom: 1px solid var(--wisp-line); } .split { grid-template-columns: 1fr; } main { padding: 1rem; } }
"#;

/// The controls: a changed one goes in the query, and the frame renders
/// the story again with it; the page's address keeps it.
const JS: &str = r#"
const form = document.getElementById('props')
const frame = document.querySelector('iframe')
let timer = 0
function render() {
  const q = new URLSearchParams()
  for (const el of form.elements) {
    if (!el.name || !el.hasAttribute('data-set')) continue
    if (el.type === 'checkbox') q.set(el.name, el.checked)
    else if (el.value !== '' || el.type === 'text') q.set(el.name, el.value)
  }
  const s = String(q)
  history.replaceState(null, '', location.pathname + (s ? '?' + s : ''))
  frame.src = frame.src.split('?')[0] + '?' + s
}
form?.addEventListener('input', (e) => {
  e.target.setAttribute('data-set', '')
  clearTimeout(timer)
  timer = setTimeout(render, 120)
})
document.getElementById('reset')?.addEventListener('click', () => {
  location.href = location.pathname
})
for (const b of document.querySelectorAll('[data-open]')) {
  b.onclick = () => fetch('/_wisp/dev/open', { method: 'POST', headers: { 'x-wisp-dev': '1' }, body: b.dataset.open })
}
"#;
