//! `src/routes/t/sugar`: an action without `->`, an `Email` input, `.await`
//! in markup and a block's name read by browser code, working together.

use wisp::test::client;
use wisp_test_app::Site;

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
            "<input aria-label=\"email\" name=\"email\" type=\"email\" required value=\"nope\"><small class=\"problem\">must be an email address</small>"
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
