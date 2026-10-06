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

/// A `---` block's literal the browser alone reads (`clicks`) is browser
/// state, as the script's `let`s are: after a form action's morph it keeps
/// what the visitor made it, and so does what derives from it. A value the
/// server computes (`count`, read as `data.count`) is the server's again.
#[test]
fn header_literals_are_browser_state() {
    let mut b = wisp::browser!(Site);
    b.goto("/a2/state");
    assert_eq!(b.text("#client"), "0");
    let seen: u32 = b.text("#seen").parse().unwrap();
    b.click("#click");
    b.click("#click");
    assert_eq!([b.text("#client"), b.text("#double")], ["2", "4"]);
    b.click("#bump");
    assert_eq!(b.text("#seen"), (seen + 1).to_string());
    assert_eq!([b.text("#client"), b.text("#double")], ["2", "4"]);
}

/// `{:#try}`: what throws in it shows its `{:catch e}` in place, the rest
/// of the page keeps working, and `reset()` draws the body again.
#[test]
fn a_try_block_recovers_in_place() {
    let mut b = wisp::browser!(Site);
    b.goto("/a2/props");
    assert_eq!(b.text("#risky"), "fine");
    b.click("#break");
    assert_eq!(b.text("#retry"), "Retry broke");
    b.click("#rekey");
    assert_eq!(b.text("#keys li"), "9", "the page still runs");
    b.click("#retry");
    assert_eq!(b.text("#risky"), "fine");
}

/// more.js, which no test page names (so the test app's binary does not
/// carry it), run in the page by hand: its helpers join live.js's, and are
/// `window.H`.
fn helpers(b: &mut wisp::test::Browser) {
    let src = include_str!("../../../crates/wisp-shared/src/client/more.js")
        .replace("import { __wisp as X } from 'wisp';", "");
    b.eval(&format!(
        "(async () => {{ const live = performance.getEntriesByType('resource').map((e) => e.name).find((n) => n.includes('/live.js'));          const X = (await import(live)).__wisp; {src}
 window.H = X.shared; }})()"
    ));
}

/// `announce`, and the actions `outside`, `shortcut`, `modal` and `preload`.
#[test]
fn helpers_and_actions() {
    let mut b = wisp::browser!(Site);
    b.goto("/a2/props");
    helpers(&mut b);
    b.eval("H.announce('Saved'); new Promise((r) => setTimeout(r, 100))");
    let said =
        b.eval("[...document.querySelectorAll('[aria-live]')].map((e) => e.textContent).join()");
    assert!(said.as_str().unwrap_or("").contains("Saved"), "{said:?}");

    b.eval("window.n = 0; const p = document.createElement('div'); p.id = 'pop'; p.textContent = 'In'; document.body.append(p); H.outside(p, () => n++)");
    b.click("#pop");
    assert_eq!(b.eval("n").as_f64(), Some(0.0), "a press inside");
    b.click("#rekey");
    assert_eq!(b.eval("n").as_f64(), Some(1.0), "a press outside");

    b.eval("window.k = 0; const h = document.createElement('button'); h.onclick = () => k++; document.body.append(h); H.shortcut(h, 'ctrl+k'); \
            document.dispatchEvent(new KeyboardEvent('keydown', { key: 'k', ctrlKey: true, bubbles: true }))");
    assert_eq!(b.eval("k").as_f64(), Some(1.0));

    b.eval("const d = document.createElement('dialog'); d.id = 'dlg'; document.body.append(d); window.M = H.modal(d, true)");
    assert_eq!(b.eval("dlg.open").as_bool(), Some(true));
    b.eval("M.update(false)");
    assert_eq!(b.eval("dlg.open").as_bool(), Some(false));

    // Every close sets the binding false (codegen hands `[open, set]`), so
    // true opens it again: Escape, a method=dialog form, close().
    b.eval("const e = document.createElement('dialog'); e.id = 'dlg2'; e.innerHTML = '<form method=dialog><button id=shut>x</button></form>'; \
            document.body.append(e); window.o = true; window.S = (v) => (o = v); window.M2 = H.modal(e, [true, S])");
    for close in [
        "press",
        "document.getElementById('shut').click()",
        "dlg2.close()",
    ] {
        assert_eq!(b.eval("dlg2.open").as_bool(), Some(true), "{close}");
        if close == "press" {
            b.press("Escape");
        } else {
            b.eval(close);
        }
        assert_eq!(
            b.eval("[dlg2.open, o].join()").as_str(),
            Some("false,false"),
            "{close}"
        );
        b.eval("M2.update([true, S])");
    }
    assert_eq!(b.eval("dlg2.open").as_bool(), Some(true));
    b.eval("M2.destroy()");

    b.eval("const a = document.createElement('a'); a.href = '/a2/stores'; a.textContent = 'ahead'; document.body.prepend(a); H.preload(a); new Promise((r) => setTimeout(r, 300))");
    let got = b.eval(
        "performance.getEntriesByType('resource').some((e) => e.name.endsWith('/a2/stores'))",
    );
    assert_eq!(got.as_bool(), Some(true));
}

/// use:keepscroll: back puts the element's scroll where it was.
#[test]
fn back_restores_kept_scroll() {
    let mut b = wisp::browser!(Site);
    b.goto("/a2/props");
    helpers(&mut b);
    let make = "{ const s = document.createElement('div'); s.id = 'side'; s.style.cssText = 'height:50px;overflow:auto'; \
                s.innerHTML = '<div style=\"height:500px\">tall</div>'; document.body.prepend(s); H.keepscroll(s) }";
    b.eval(make);
    b.eval("side.scrollTop = 120");
    b.eval(
        "document.dispatchEvent(new CustomEvent('wisp:goto', { detail: { url: '/a2/state' } }))",
    );
    b.wait("#click");
    b.eval("history.back()");
    b.wait("#rekey");
    helpers(&mut b);
    b.eval(make);
    assert_eq!(b.eval("side.scrollTop").as_f64(), Some(120.0));
}
