//! The serve extras through the whole request path: compression, `Range`,
//! the health check and the security headers, on files and pages as the
//! server answers them.

use crate::compress::tests::inflate;
use crate::{App, Cx, Out, Request};

struct Site;

impl App for Site {
    const ROOT: &'static str = ".";
    const CSS: Option<&'static str> = None;
    const ROUTES: &'static [crate::rt::RouteFacts] = &[crate::rt::RouteFacts::new(&[])];
    const TEMPLATES: &'static [(&'static str, u64)] = &[];

    fn route(path: &str) -> Option<(usize, [&str; 8])> {
        (path == "/").then_some((0, [""; 8]))
    }

    fn shell() -> [&'static str; 3] {
        ["", "", ""]
    }

    fn asset(_: &str) -> Option<&'static crate::Asset> {
        None
    }

    async fn init() -> crate::Result<()> {
        Ok(())
    }

    async fn handle(route: Option<usize>, _: &mut Cx, out: &mut Out) -> crate::Result<()> {
        if route.is_none() {
            return Err(crate::Error::new(404, "Not Found"));
        }
        out.body.push_str("hello");
        Ok(())
    }

    async fn error(
        _: Option<usize>,
        _: &mut Cx,
        out: &mut Out,
        _: u16,
        _: &str,
    ) -> crate::Result<()> {
        out.body.push_str("oops");
        Ok(())
    }
}

const JS: &str = crate::protocol::WISP_JS_PATH;

fn get(with: &[(&str, &str)]) -> crate::Reply {
    let mut app = crate::test::client::<Site>();
    let mut req = Request::new("GET", JS);
    for (n, v) in with {
        req.header(n, v);
    }
    app.send(req)
}

#[test]
fn a_file_is_gzipped_for_a_client_that_takes_it() {
    let plain = get(&[]);
    assert_eq!(plain.status, 200);
    assert_eq!(plain.header("content-encoding"), None);
    assert_eq!(plain.header("vary"), Some("accept-encoding"));
    let gz = get(&[("accept-encoding", "br, gzip")]);
    assert_eq!(gz.header("content-encoding"), Some("gzip"));
    assert_eq!(gz.header("vary"), Some("accept-encoding"));
    assert!(gz.bytes().len() < plain.bytes().len());
    assert_eq!(inflate(gz.bytes()), plain.bytes());
    let no = get(&[("accept-encoding", "gzip;q=0")]);
    assert_eq!(no.header("content-encoding"), None);
}

#[test]
fn a_range_gets_those_bytes() {
    let whole = get(&[]);
    let n = whole.bytes().len();
    assert_eq!(whole.header("accept-ranges"), Some("bytes"));
    let part = get(&[("range", "bytes=2-9")]);
    assert_eq!(part.status, 206);
    assert_eq!(
        part.header("content-range"),
        Some(&*format!("bytes 2-9/{n}"))
    );
    assert_eq!(part.bytes(), &whole.bytes()[2..10]);
    let tail = get(&[("range", "bytes=-5")]);
    assert_eq!(tail.status, 206);
    assert_eq!(tail.bytes(), &whole.bytes()[n - 5..]);
    // A range is of the file as it is, not of its gzip.
    let both = get(&[("range", "bytes=0-3"), ("accept-encoding", "gzip")]);
    assert_eq!((both.status, both.header("content-encoding")), (206, None));
    assert_eq!(both.bytes(), &whole.bytes()[..4]);
}

#[test]
fn a_range_the_file_lacks_is_a_416() {
    let n = get(&[]).bytes().len();
    let r = get(&[("range", &format!("bytes={n}-"))]);
    assert_eq!(r.status, 416);
    assert_eq!(r.header("content-range"), Some(&*format!("bytes */{n}")));
    assert!(r.bytes().is_empty());
}

#[test]
fn what_is_not_one_range_is_the_whole_file() {
    let whole = get(&[]);
    for h in ["bytes=0-1,4-5", "lines=1-2", "bytes=5-2"] {
        let r = get(&[("range", h)]);
        assert_eq!(r.status, 200, "{h}");
        assert_eq!(r.bytes(), whole.bytes());
    }
    // `if-range` names another version of the file than this one.
    let r = get(&[("range", "bytes=0-3"), ("if-range", "\"old\"")]);
    assert_eq!(r.status, 200);
    let tag = whole.header("etag").unwrap().to_owned();
    let r = get(&[("range", "bytes=0-3"), ("if-range", &tag)]);
    assert_eq!(r.status, 206);
}

#[test]
fn health_says_ok() {
    let mut app = crate::test::client::<Site>();
    let r = app.get("/_wisp/health");
    assert_eq!((r.status, r.text()), (200, "ok"));
    assert_eq!(r.header("cache-control"), Some("no-store"));
}

#[test]
fn pages_carry_security_headers() {
    let mut app = crate::test::client::<Site>();
    let page = app.get("/");
    assert_eq!(page.status, 200);
    assert_eq!(page.header("x-content-type-options"), Some("nosniff"));
    assert_eq!(
        page.header("referrer-policy"),
        Some("strict-origin-when-cross-origin")
    );
    // The error page too.
    let gone = app.get("/nothing");
    assert_eq!(gone.status, 404);
    assert_eq!(gone.header("x-content-type-options"), Some("nosniff"));
    assert_eq!(page.header("strict-transport-security"), None);
}
