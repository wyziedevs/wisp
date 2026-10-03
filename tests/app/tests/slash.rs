//! `wisp::trailing_slash`, set as `init` would: in a process of its own,
//! since it holds for the whole app.

use wisp::TrailingSlash::{Always, Ignore, Never};
use wisp::test::client;
use wisp_test_app::Site;

#[test]
fn pages_end_as_trailing_slash_says() {
    let mut app = client::<Site>();
    // `Never`, the default: a page's address with a `/` gets a 308 without.
    let r = app.get("/login/?a=1");
    assert_eq!((r.status, r.header("location")), (308, Some("/login?a=1")));

    wisp::trailing_slash(Ignore);
    assert_eq!(app.get("/login/").status, 200);
    assert_eq!(app.get("/login").status, 200);

    wisp::trailing_slash(Always);
    let r = app.get("/login?a=1");
    assert_eq!((r.status, r.header("location")), (308, Some("/login/?a=1")));
    assert_eq!(app.get("/login/").status, 200);
    assert_eq!(app.get("/").status, 200);
    // Endpoints, files and what is no route's are left as they are asked for.
    assert_ne!(app.get("/echo").status, 308);
    assert_eq!(app.get("/_app/wisp.js").status, 200);
    assert_eq!(app.get("/nope").status, 404);
    let r = app.get("/nope/");
    assert_eq!((r.status, r.header("location")), (308, Some("/nope")));

    wisp::trailing_slash(Never);
    assert_eq!(app.get("/login/").status, 308);
    assert_eq!(app.get("/login").status, 200);
}
