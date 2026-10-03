//! Layout resets and inherited options, typed routes, `reroute`, and the
//! origin check of `+server.rs` endpoints.

mod common;

use wisp::test::client;
use wisp_test_app::Site;

#[test]
fn pages_inherit_a_layouts_cache_and_can_leave_its_layouts() {
    // `CACHE_PUBLIC` of the layout keeps the page: the second answer is the
    // first (out of dev mode, which keeps nothing).
    let s = common::start(&[("WISP_DEV", "off"), ("WISP_THREADS", "1")]);
    let first = s.request("GET", "/t/grp/inner", "", b"");
    let again = s.request("GET", "/t/grp/inner", "", b"");
    assert!(first.contains("<p>inner</p>") && first.contains("data-grp=\"0\""), "{first}");
    assert_eq!(common::body(&first), common::body(&again));
    let mut app = client::<Site>();
    // `+page@.wisp` is without the layout.
    let bare = app.get("/t/grp/bare").text().to_string();
    assert!(bare.contains("<p>bare</p>") && !bare.contains("data-grp"), "{bare}");
}

#[test]
fn routes_are_typed_functions() {
    let mut app = client::<Site>();
    let page = app.get("/t/grp/links").text().to_string();
    for want in ["href=\"/post/a%20b%2Fc\"", "href=\"/\"", "href=\"/t/grp/inner\""] {
        assert!(page.contains(want), "{want}\n{page}");
    }
}

#[test]
fn reroute_picks_the_route_and_costs_nothing_to_the_rest() {
    let mut app = client::<Site>();
    assert_eq!(app.get("/rr/t/grp/bare").status, 200);
    assert_eq!(app.get("/rr").status, 404);
    assert_eq!(app.get("/t/grp/bare").status, 200);
}

#[test]
fn endpoints_refuse_another_sites_unsafe_requests() {
    let mut app = client::<Site>();
    assert_eq!(app.post_json("/echo", "x").status, 200, "no Origin: a script, curl");
    app.header("origin", "https://evil.example");
    let r = app.post_json("/echo", "x");
    assert_eq!(r.status, 403, "{}", r.text());
    // A GET is not a change.
    app.header("origin", "https://evil.example");
    assert_ne!(app.get("/echo").status, 403);
}
