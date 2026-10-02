//! `src/routes/t/sugar`: an action without `->`, an `Email` input, `.await`
//! in markup and a block's name read by browser code, working together.

use wisp::test::client;
use wisp_test_app::Site;

#[test]
fn what_app_code_leaves_out() {
    let mut app = client::<Site>();
    assert!(app.get("/t/sugar").text().contains("<p id=\"count\">0</p>"));

    // Not an email: the page again, what was typed and the problem by it.
    let bad = app.post_form("/t/sugar?/join", &[("email", "nope")]);
    assert_eq!(bad.status, 422);
    assert!(
        bad.text().contains(
            "<input name=\"email\" type=\"email\" required value=\"nope\"><small class=\"problem\">must be an email address</small>"
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
