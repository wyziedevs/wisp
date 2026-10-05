//! `src/routes/t/sugar`: an action without `->`, an `Email` input, `.await`
//! in markup and a block's name read by browser code, working together.

#![cfg(not(target_arch = "wasm32"))]

use wisp::test::client;
use wisp_test_app::Site;

/// `wisp add form /t/contact` wrote that page: it posts, flashes, shows it once.
#[test]
fn a_generated_form_flashes_once() {
    let mut app = client::<Site>();
    let sent = app.post_form(
        "/t/contact",
        &[("name", "Ann"), ("email", "a@b.co"), ("topic", "help")],
    );
    assert_eq!(sent.status, 303);
    let page = app.get("/t/contact").text().to_string();
    assert!(
        page.contains("<p class=\"flash\" role=\"status\">Thanks, Ann! We will write to a@b.co about help.</p>"),
        "{page}"
    );
    let meta = "<meta name=\"description\" content=\"Write to us\"><meta property=\"og:description\" content=\"Write to us\"><meta property=\"og:image\" content=\"/og.png\">";
    assert!(page.contains(meta), "{page}");
    let select = "<select name=\"topic\"><option>hello</option><option>help</option></select>";
    assert!(page.contains(select), "{page}");
    let bad = app.post_form(
        "/t/contact",
        &[("name", "A"), ("email", "a@b.co"), ("topic", "x")],
    );
    assert_eq!(bad.status, 422);
    assert!(
        bad.text().contains("must be one of hello, help"),
        "{}",
        bad.text()
    );
    assert!(!app.get("/t/contact").text().contains("flash"));
}

/// `wisp add crud /t/memos` wrote these pages and `src/db.rs`.
#[test]
fn generated_crud_creates_edits_and_deletes() {
    let mut app = client::<Site>();
    assert!(app.get("/t/memos").text().contains("None yet."));
    let new = app.get("/t/memos/new").text().to_string();
    assert!(new.contains("<button>Create</button></form>"), "{new}");
    let r = app.post_form("/t/memos/new", &[("title", "One"), ("body", "B")]);
    assert_eq!(r.status, 303);
    let list = app.get("/t/memos").text().to_string();
    assert!(
        list.contains("Created") && list.contains(">One</a>"),
        "{list}"
    );
    let end = list.find("/edit\"").unwrap();
    let id: u64 = list[list[..end].rfind('/').unwrap() + 1..end]
        .parse()
        .unwrap();
    let edit = app.get(&format!("/t/memos/{id}/edit")).text().to_string();
    assert!(
        edit.contains("value=\"One\"") && edit.contains("<button>Save</button>"),
        "{edit}"
    );
    let r = app.post_form(
        &format!("/t/memos/{id}/edit"),
        &[("title", "Two"), ("body", "B")],
    );
    assert_eq!(r.status, 303);
    assert!(app.get("/t/memos").text().contains(">Two</a>"));
    app.post_form(&format!("/t/memos?/remove&id={id}"), &[]);
    assert!(app.get("/t/memos").text().contains("None yet."));
    assert_eq!(app.get("/t/memos/999/edit").status, 404);
}

#[test]
fn an_active_link_is_the_current_page() {
    let mut app = client::<Site>();
    let page = app.get("/a2/nav/two").text().to_string();
    assert!(
        page.contains("<a id=\"one\" href=\"/a2/nav/one\">"),
        "{page}"
    );
    let two = "<a id=\"two\" href=\"/a2/nav/two\" aria-current=\"page\">";
    assert!(page.contains(two), "{page}");
}

#[test]
fn a_form_asks_for_what_its_action_takes() {
    let mut app = client::<Site>();
    let page = app.get("/t/fields").text().to_string();
    for want in [
        "<form method=\"post\" enctype=\"multipart/form-data\">",
        "<label>Email <input name=\"email\" type=\"email\" required></label>",
        "<input name=\"password\" type=\"password\" required minlength=\"8\">",
        "<label>Note <input name=\"note\"",
        "<input name=\"avatar\" type=\"file\" accept=\"image/*\">",
        "<button>Go</button></form>",
    ] {
        assert!(page.contains(want), "{want}\n{page}");
    }
    let bad = app.post_form("/t/fields", &[("email", "nope"), ("password", "x")]);
    assert_eq!(bad.status, 422);
    assert!(bad.text().contains("value=\"nope\""), "{}", bad.text());
}

#[test]
fn what_app_code_leaves_out() {
    let mut app = client::<Site>();
    assert!(app.get("/t/sugar").text().contains("<p id=\"count\">0</p>"));

    // Not an email: the page again, what was typed and the problem by it.
    let bad = app.post_form("/t/sugar?/join", &[("email", "nope")]);
    assert_eq!(bad.status, 422);
    assert!(
        bad.text().contains(
            "<input aria-label=\"email\" name=\"email\" type=\"email\" required value=\"nope\" aria-invalid=\"true\"><small class=\"problem\">must be an email address</small>"
        ),
        "{}",
        bad.text()
    );

    // `return error(..)`, and a bare `return`, from an action without `->`.
    let zero = app.post_form("/t/sugar?/join", &[("email", "a@b.co"), ("n", "0")]);
    assert_eq!(zero.status, 400);
    let one = app.post_form("/t/sugar?/join", &[("email", "a@b.co"), ("n", "1")]);
    assert_eq!(one.status, 200);

    // Its last line, `redirect(..)`, is what it returns.
    let ok = app.post_form("/t/sugar?/join", &[("email", "ann@example.com")]);
    assert_eq!(ok.status, 303);
    let page = app.get("/t/sugar").text().to_string();
    assert!(page.contains("<p id=\"count\">1</p>"), "{page}");
    // `last` in browser code is the block's `last`, sent by its name.
    assert!(page.contains("\"last\":\"ann@example.com 2\""), "{page}");
}

#[test]
fn a_route_limits_and_times_itself() {
    let mut app = client::<Site>();
    // `const RATE_LIMIT: u32 = 2;`: the third request in a minute is a 429.
    assert_eq!(app.get("/t/limited").status, 200);
    assert_eq!(app.get("/t/limited").status, 200);
    assert_eq!(app.get("/t/limited").status, 429);
    // `const TIMEOUT: u32 = 1;`.
    assert_eq!(app.get("/t/stuck").status, 503);
}

#[test]
fn named_middleware_runs_first_in_a_page_and_a_layout() {
    let mut app = client::<Site>();
    // `const MIDDLEWARE: &[&str] = &["gate", "stamp"];` (src/middleware.rs).
    assert_eq!(app.get("/t/gated").status, 403);
    let ok = app.get("/t/gated?key=open");
    assert_eq!(ok.status, 200);
    assert_eq!(ok.header("x-stamped"), Some("yes"));
    // In a layout: every page below.
    assert_eq!(app.get("/t/gate").status, 403);
    assert_eq!(app.get("/t/gate?key=open").status, 200);
}

#[test]
fn client_only_sends_its_fallback_and_keeps_the_children_inert() {
    let mut app = client::<Site>();
    // The ClientOnly component (`wisp ui add clientonly`): children in a
    // `<template>`, drawn on mount; what is visible is the fallback.
    let page = app.get("/t/clientonly").text().to_string();
    assert!(
        page.contains("<template data-w=\"0.0\"><p>secret inside</p></template>"),
        "{page}"
    );
    assert!(
        page.contains(
            "<span class=\"client-only\"><template data-w=\"2\"></template>wait<!----></span>"
        ),
        "{page}"
    );
}

#[test]
fn a_slot_is_drawn_inside_its_layout_with_its_own_load() {
    let mut app = client::<Site>();
    // `@stats/+page.wisp` (+ `+page.rs`) beside the layout's `{@render stats()}`.
    for path in ["/t/dash", "/t/dash/more"] {
        let page = app.get(path).text().to_string();
        assert!(
            page.contains("<aside><p>stats 42</p>\n</aside>")
                || page.contains("<aside><p>stats 42</p></aside>"),
            "{path}: {page}"
        );
    }
    assert!(app.get("/t/dash/more").text().contains("<h2>more</h2>"));
}

#[test]
fn an_intercepting_page_is_a_fragment_for_the_slot_and_a_reload_is_the_page() {
    let mut app = client::<Site>();
    // The layout draws `@modal` in a place wisp.js finds, and says what it shows.
    let list = app.get("/t/gal").text().to_string();
    let cut = "<div data-wisp-cut=\"[[&quot;/t/gal/item/[id]&quot;,&quot;/t/gal/@modal/(.)item/[id]&quot;]]\"></div>";
    assert!(list.contains(cut), "{list}");
    // What a client navigation to `/t/gal/item/7` fetches: no layouts.
    let part = app.get("/t/gal/@modal/(.)item/7").text().to_string();
    assert!(
        part.contains("<dialog open><p>modal 7</p></dialog>"),
        "{part}"
    );
    assert!(!part.contains("data-wisp-cut"), "{part}");
    // The address itself, loaded whole, is the route's own page.
    let full = app.get("/t/gal/item/7").text().to_string();
    assert!(
        full.contains("<h1>item 7</h1>") && !full.contains("<dialog"),
        "{full}"
    );
}

#[test]
fn a_layout_can_be_for_members() {
    let mut app = client::<Site>();
    // `const SIGNED_IN: bool = true;` in the layout: pages and actions.
    assert_eq!(app.get("/t/members").status, 303);
    assert_eq!(app.post_form("/t/members?/poke", &[]).status, 303);
    assert_eq!(
        app.post_form("/t/members?/poke", &[]).location(),
        Some("/login")
    );
    app.sign_in(1);
    assert_eq!(app.get("/t/members").status, 200);
    assert_eq!(
        app.post_form("/t/members?/poke", &[]).location(),
        Some("/t/members")
    );
}

#[test]
fn config_reads_the_environment() {
    let mut app = client::<Site>();
    let page = app.get("/t/conf").text().to_string();
    assert!(page.contains("<p id=\"conf\">false true</p>"), "{page}");
}
