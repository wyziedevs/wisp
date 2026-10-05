//! Client features in a headless Chrome or Edge (`cargo test -p
//! wisp-test-app --features browser`; skipped where there is none).
#![cfg(not(target_arch = "wasm32"))]
#![cfg(feature = "browser")]

use wisp_test_app::Site;

/// A link to a download: the page stays, and `navigating` is cleared.
#[test]
fn a_download_link_ends_the_navigation() {
    let mut b = wisp::browser!(Site);
    b.goto("/a2/shallow");
    assert_eq!(b.text("#nav"), "here");
    b.click("#save");
    b.eval("new Promise((r) => setTimeout(r, 300))");
    assert!(b.url().ends_with("/a2/shallow"), "{}", b.url());
    assert_eq!(b.text("#nav"), "here");
}

/// pushState and replaceState: `page.value.state` follows the history,
/// and back and forward to an entry made so ask the server nothing.
#[test]
fn shallow_routing() {
    let mut b = wisp::browser!(Site);
    b.goto("/a2/shallow");
    let n = b.text("#n");
    assert_eq!(b.text("output"), "1");
    b.click("#two");
    assert_eq!(b.text("output"), "2");
    assert!(b.url().ends_with("/a2/shallow?tab=2"), "{}", b.url());
    b.eval("history.back()");
    assert_eq!(b.text("output"), "1");
    assert!(b.url().ends_with("/a2/shallow"));
    b.eval("history.forward()");
    assert_eq!(b.text("output"), "2");
    b.click("#three");
    assert_eq!(b.text("output"), "3");
    assert!(
        b.url().ends_with("?tab=2"),
        "replaceState('') keeps the URL"
    );
    b.eval("history.back()");
    assert_eq!(b.text("output"), "1");
    b.eval("history.forward()");
    assert_eq!(b.text("output"), "3");
    assert_eq!(b.text("#n"), n, "no request");

    // Away and back: that entry's page from the server, with its state.
    b.click("#away");
    assert_eq!(b.text("h1"), "Islands");
    b.eval("history.back()");
    assert_eq!(b.text("output"), "3");
    assert_ne!(b.text("#n"), n);
}

/// Snapshots: fields, and a script's `export const snapshot`, come back
/// with their history entry, by back and by a reload; passwords and
/// `autocomplete="off"` never do.
#[test]
fn snapshots() {
    let mut b = wisp::browser!(Site);
    b.goto("/a2/snap");
    for (sel, text) in [
        ("#text", "kept"),
        ("#secret", "pw"),
        ("#off", "no"),
        ("#note", "hi"),
    ] {
        b.fill(sel, text);
    }
    b.click("#box");
    b.eval("pick.value = 'b'");
    b.click("#more");
    b.click("#more");
    assert_eq!(b.text("#more"), "More 2");
    let fields =
        "[text.value, secret.value, off.value, box.checked, pick.value, note.value].join()";
    let want = "kept,,,true,b,hi";

    b.click("#away");
    assert_eq!(b.text("h1"), "Islands");
    b.eval("history.back()");
    assert_eq!(b.text("h1"), "Snap");
    assert_eq!(b.eval(fields).as_str(), Some(want));
    assert_eq!(b.text("#more"), "More 2");

    b.eval("document.body.dataset.old = 1; location.reload()");
    b.wait("body:not([data-old]) #more");
    assert_eq!(b.eval(fields).as_str(), Some(want));
    assert_eq!(b.text("#more"), "More 2");
}

/// An island in a server component (no script: no JS) in an island, as
/// the outer one's children and in its own markup.
#[test]
fn islands_inside_server_components_inside_islands() {
    let mut b = wisp::browser!(Site);
    b.goto("/a2/server");
    for at in ["#slotted", "#direct"] {
        b.wait(&format!("{at} .ping[data-on]"));
        b.click(&format!("{at} .outer"));
        assert_eq!(b.text(&format!("{at} .outer")), "Outer 1");
        b.click(&format!("{at} .tally"));
        assert_eq!(b.text(&format!("{at} .tally")), "1");
    }
    // The inner island wakes at its own moment, and wakes the outer one,
    // which waits for a touch, with it.
    b.wait("#lazy .ping[data-on]");
    b.click("#lazy .outer");
    assert_eq!(b.text("#lazy .outer"), "Outer 1");
    b.click("#lazy .tally");
    assert_eq!(b.text("#lazy .tally"), "1");
}

/// `{#await}`: each answer takes its pending markup's place, and its
/// components start, on a load and on a navigation wisp.js makes.
#[test]
fn awaits_answer_in_place() {
    let mut b = wisp::browser!(Site);
    let placed =
        "[...document.querySelectorAll('h1 ~ p, #c > p')].map((p) => p.id || p.textContent).join()";
    let want = "a,b,Something went wrong,d,end";
    let left =
        "wisp-await, [data-wisp-await], [data-wisp-live], body > script:not([src]):not([type])";
    // Components in an answer start with the page's: each its own.
    let live = |b: &mut wisp::test::Browser| {
        b.click("#e .tally");
        b.click("#e .tally");
        assert_eq!(b.text("#e .tally"), "2");
        b.click("#top .tally");
        assert_eq!(b.text("#top .tally"), "1");
        assert_eq!(b.text("#e .tally"), "2");
        b.click("#e .step"); // an island, started when idle
        assert_eq!(b.text("#e .step"), "6");
    };
    b.goto("/await");
    assert_eq!(b.text("#a"), "Got 7");
    assert_eq!(b.eval(placed).as_str(), Some(want));
    assert_eq!(b.count(left), 0);
    live(&mut b);
    b.goto("/await/plain");
    assert_eq!(b.text("#w"), "later");
    b.click("#go");
    assert_eq!(b.text("#a"), "Got 7");
    assert_eq!(b.eval(placed).as_str(), Some(want));
    assert_eq!(b.count(left), 0);
    live(&mut b);
}

/// A component the browser draws takes snippets as props (passed by name,
/// and among its children), and `{:@const}` and `{:@html}` draw in place.
#[test]
fn components_draw_the_snippets_they_are_given() {
    let mut b = wisp::browser!(Site);
    b.goto("/a2/snippets");
    assert_eq!(b.count("li"), 4);
    b.click("button");
    assert_eq!(b.count("li"), 6);
    let text = "[...document.querySelectorAll('li')].map((e) => e.textContent.trim()).join()";
    assert_eq!(
        b.eval(text).as_str(),
        Some("1. plum,2. fig,3. pear,PLUM,FIG,PEAR")
    );
    assert_eq!(b.text("#note i"), "raw");
}

/// A navigation into a `@slot` (the gallery's modal) leaves the page where
/// it is, and `navigating` is cleared.
#[test]
fn a_slot_navigation_ends_the_navigation() {
    let mut b = wisp::browser!(Site);
    b.goto("/t/gal");
    b.click("a[href='/t/gal/item/7']");
    b.eval("new Promise((r) => setTimeout(r, 300))");
    assert_eq!(b.text("dialog"), "modal 7");
    assert_eq!(b.text("#nav"), "here");
}

/// A form post with JS on: the 422 morphs in (no load, no history entry),
/// each problem beside its input and announced by it, what was typed kept
/// with the focus and the caret, a second submit while one is out dropped,
/// the same markup as without JS, a file sent as multipart, a redirect
/// after a post followed, and a post the network loses handed to the
/// browser.
#[test]
fn forms_morph_in_place() {
    let mut b = wisp::browser!(Site);
    b.goto("/t/forms");
    b.goto("/t/forms2");
    let length = b.eval("history.length");
    b.eval("window.__posts = 0; addEventListener('wisp:submit', () => __posts++)");
    // (`name` alone is `window.name`.)
    let n = "document.getElementById('name2')";
    b.fill("[aria-label=note2]", "typed");
    b.fill("#name2", "twice");
    b.eval(&format!("{n}.setSelectionRange(1, 1)"));
    // Enter in a field submits its form; the page morphs, not loads (a
    // load would lose `__posts`).
    b.press("Enter");
    b.wait("#name2-problem");
    assert_eq!(
        b.eval("history.length"),
        length,
        "no history entry for a 422"
    );
    assert!(b.url().ends_with("/t/forms2"), "{}", b.url());
    assert_eq!(b.text("#name2-problem"), "first");
    assert_eq!(b.text("#name2-problem + small"), "second");
    assert_eq!(b.attr("#name2", "aria-invalid").as_deref(), Some("true"));
    assert_eq!(
        b.attr("#name2", "aria-describedby").as_deref(),
        Some("name2-problem")
    );
    assert_eq!(
        b.eval(&format!(
            "[{n}.value, document.querySelector('[aria-label=note2]').value, document.activeElement.id, {n}.selectionStart].join()"
        ))
        .as_str(),
        Some("twice,typed,name2,1"),
        "kept, focused, caret where it was"
    );
    assert_eq!(b.eval("__posts").as_i64(), Some(1));
    // The first form is its own still.
    assert_eq!(
        b.eval("document.getElementById('name').value").as_str(),
        Some("Ada")
    );
    assert_eq!(b.count("#join [aria-invalid]"), 0);

    // Twice at once: one post.
    b.eval("other.requestSubmit(); other.requestSubmit()");
    b.wait("#name2-problem");
    assert_eq!(b.eval("__posts").as_i64(), Some(2));

    // The page is what the server sends without JS: same form, same bytes.
    let served = b.eval(
        "fetch('/t/forms2?/other', { method: 'POST', body: new URLSearchParams(new FormData(other)) }).then((r) => r.text())\
         .then((h) => new DOMParser().parseFromString(h, 'text/html').getElementById('other').outerHTML)",
    );
    assert_eq!(b.eval("other.outerHTML"), served, "JS on and off agree");

    // The rules pass; the action wants the tick: its problem by the box.
    // A file goes along, multipart (the form says so).
    b.fill("#name", "Al");
    b.eval(
        "const dt = new DataTransfer(); dt.items.add(new File(['hello'], 'a.txt', { type: 'text/plain' }));\
         document.querySelector('[name=doc]').files = dt.files",
    );
    b.click("#join button");
    b.wait("#agree[aria-invalid]");
    assert_eq!(b.text("#agree + small"), "Tick to accept the terms");
    assert_eq!(b.count("#name-problem"), 0);
    assert_eq!(
        b.eval("document.getElementById('name').value").as_str(),
        Some("Al")
    );
    assert_eq!(b.eval("__posts").as_i64(), Some(3));

    // Ticked, it passes: the redirect is followed, as a load would be.
    b.click("#agree");
    b.click("#join button");
    assert_eq!(b.count("small.problem"), 0);
    assert_eq!(
        b.eval("document.getElementById('name').value").as_str(),
        Some("Ada"),
        "the page anew"
    );
    assert!(b.url().ends_with("/t/forms2"), "{}", b.url());
    // The history is the browser's own: the post made an entry (a 303 to
    // the same page would too), so back is the page before the post.
    b.eval("history.back()");
    assert!(b.url().ends_with("/t/forms2"), "{}", b.url());
    assert_eq!(b.count("small.problem"), 0);
    assert_eq!(
        b.eval("history.length").as_i64(),
        length.as_i64().map(|n| n + 1)
    );

    // The network loses the post: the browser sends it itself, and shows
    // the server's answer as a loaded page.
    b.goto("/t/forms2");
    b.eval(
        "window.__mark = 1; const f = fetch;\
         window.fetch = (...a) => { window.fetch = f; return Promise.reject(new TypeError('lost')) }",
    );
    b.fill("#name2", "B");
    b.click("#other button");
    b.wait("#name2-problem");
    assert_eq!(
        b.eval("typeof __mark").as_str(),
        Some("undefined"),
        "a load"
    );
    assert_eq!(b.eval(&format!("{n}.value")).as_str(), Some("B"));
}

/// A `---` block's literals the browser alone reads are browser state, as a
/// script's `let`s are: the page has no script, and after a form action's
/// morph they keep what the visitor made them. A value the server computes
/// (`bumps`) is the server's again after the morph.
#[test]
fn header_literals_are_browser_state() {
    let mut b = wisp::browser!(Site);
    b.goto("/a2/header");
    assert_eq!(b.text("#clicks"), "5");
    assert_eq!(b.text("#on"), "off");
    assert_eq!(b.text("#who"), "me");
    assert_eq!(b.text("#list"), "2");
    let bumps: u32 = b.text("#bumps").parse().unwrap();
    b.click("#click");
    b.click("#click");
    b.click("#toggle");
    b.click("#add");
    b.click("#local");
    b.fill("#who-in", "you");
    assert_eq!(b.text("#clicks"), "7");
    assert_eq!(b.text("#on"), "on");
    assert_eq!(b.text("#who"), "you");
    assert_eq!(b.text("#list"), "3");
    assert_eq!(b.text("#bumps"), (bumps + 10).to_string());
    b.click("#bump");
    assert_eq!(b.text("#bumps"), (bumps + 1).to_string());
    assert_eq!(b.text("#clicks"), "7");
    assert_eq!(b.text("#on"), "on");
    assert_eq!(b.text("#who"), "you");
    assert_eq!(b.text("#list"), "3");
}
