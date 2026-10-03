//! Client features in a headless Chrome or Edge (`cargo test -p
//! wisp-test-app --features browser`; skipped where there is none).
#![cfg(feature = "browser")]

use wisp_test_app::Site;

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
