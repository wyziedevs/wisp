//! `src/routes/one`, a route in one `.wisp` (data, an action and a `mod
//! server` of endpoints in its block), answers as `src/routes/three`, the
//! same route in `+page.wisp`, `+page.rs` and `+server.rs`, does.

#![cfg(not(target_arch = "wasm32"))]

use wisp::test::client;
use wisp_test_app::Site;

#[test]
fn one_file_answers_as_three_do() {
    let mut app = client::<Site>();
    let mut seen = Vec::new();
    for at in ["/one", "/three"] {
        let first = app.get(at).text().to_string();
        assert!(first.contains("<p>n = 0</p>"), "{first}");
        let page = app.post_form(&format!("{at}?/add"), &[("by", "2")]);
        assert_eq!(page.status, 200);
        let page = page.text().to_string();
        assert!(page.contains("<p>n = 2</p>"), "{page}");
        assert_eq!(
            app.post_form(&format!("{at}?/add"), &[("by", "x")]).status,
            422
        );
        let put = app.put_json(at, r#"{"text":"hi"}"#);
        assert_eq!((put.status, put.text()), (200, "2"));
        let del = app.delete(&format!("{at}/21"));
        assert_eq!((del.status, del.text()), (200, "42"));
        assert_eq!(app.delete(at).status, 405);
        seen.push(page.replace(at, ""));
    }
    assert_eq!(seen[0], seen[1]);
}
