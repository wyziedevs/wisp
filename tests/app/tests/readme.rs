//! The page in the README, as the README shows it (`src/routes/t/readme`):
//! a counter kept in a cookie, changed by an action.

#![cfg(not(target_arch = "wasm32"))]

use wisp::test::client;
use wisp_test_app::Site;

#[test]
fn the_readme_counter_counts_and_stops_at_ten() {
    let mut app = client::<Site>();
    let first = app.get("/t/readme");
    assert_eq!(first.status, 200);
    assert!(
        first.text().contains("<h1>Clicked 0 times</h1>"),
        "{}",
        first.text()
    );
    assert!(
        first
            .text()
            .contains("<form action=\"?/add\" method=\"post\">"),
        "{}",
        first.text()
    );
    assert!(!first.text().contains("disabled"));

    for n in 1..=9 {
        let page = app.post_form("/t/readme?/add", &[("by", "1")]);
        assert_eq!(page.status, 200);
        assert!(
            page.text().contains(&format!("Clicked {n} times")),
            "{n}: {}",
            page.text()
        );
    }
    // What the action stored is what the page reads, in the same request and after.
    assert_eq!(app.cookie("count"), Some("9"));
    let tenth = app.post_form("/t/readme?/add", &[("by", "1")]);
    assert!(tenth.text().contains("Clicked 10 times"));
    assert!(tenth.text().contains("disabled"), "{}", tenth.text());
    assert!(app.get("/t/readme").text().contains("disabled"));

    // The input is checked by its type: not a number is the page again, a
    // 422 with the problem by field; missing is a 400.
    let bad = app.post_form("/t/readme?/add", &[("by", "lots")]);
    assert_eq!(bad.status, 422);
    assert!(bad.text().contains("Clicked 10 times"), "{}", bad.text());
    let missing = app.post_form("/t/readme?/add", &[]);
    assert_eq!(missing.status, 400);
    // An action that does not exist is a 404, and a page takes no other verb.
    assert_eq!(app.post_form("/t/readme?/nope", &[("by", "1")]).status, 404);
    assert_eq!(app.delete("/t/readme").status, 405);
    // A form from another site is refused before the action runs.
    let mut req = wisp::Request::new("POST", "/t/readme?/add");
    req.header("host", "lab.example");
    req.header("origin", "https://evil.example");
    req.header("content-type", "application/x-www-form-urlencoded");
    req.body = b"by=5".to_vec();
    assert_eq!(app.send(req).status, 403);
    assert_eq!(app.cookie("count"), Some("10"));
}
