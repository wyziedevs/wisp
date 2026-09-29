//! The app as a tower service, and a tower service behind the app (the
//! `tower` feature): bodies in frames, limits, peers, headers both ways.
#![cfg(feature = "tower")]

mod common;
#[path = "../../../tests/shared/tower.rs"]
mod shared;

use common::Lab;
use shared::{run, text};
use std::pin::Pin;
use std::task::{Context, Poll};
use wisp::tower::http::{self, Request, Response};
use wisp::tower::http_body::{Body as _, Frame, SizeHint};
use wisp::tower::tower_service::Service as _;
use wisp::tower::{Body, Bytes};

/// A request body in the frames it is given, and how it ends.
struct Frames {
    frames: Vec<Result<Frame<Bytes>, &'static str>>,
}

impl wisp::tower::http_body::Body for Frames {
    type Data = Bytes;
    type Error = &'static str;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        _: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Bytes>, &'static str>>> {
        Poll::Ready((!self.frames.is_empty()).then(|| self.frames.remove(0)))
    }

    fn size_hint(&self) -> SizeHint {
        SizeHint::default()
    }
}

fn data(s: &'static str) -> Result<Frame<Bytes>, &'static str> {
    Ok(Frame::data(Bytes::from_static(s.as_bytes())))
}

fn frames(frames: Vec<Result<Frame<Bytes>, &'static str>>) -> Frames {
    Frames { frames }
}

#[test]
fn requests_are_read_from_their_frames() {
    run(async {
        let mut svc = wisp::tower::service::<Lab>().await.unwrap();
        let mut ask = async |method: &str, uri: &str, body: Frames| {
            let req = Request::builder()
                .method(method)
                .uri(uri)
                .body(body)
                .unwrap();
            let res = svc.call(req).await.unwrap();
            (res.status().as_u16(), text(res.into_body()).await)
        };
        // In pieces, with the trailers a body may end with.
        let pieces = frames(vec![
            data("hel"),
            data("lo "),
            data("world"),
            Ok(Frame::trailers(http::HeaderMap::new())),
        ]);
        assert_eq!(
            ask("POST", "/echo", pieces).await,
            (200, "11:hello world".into())
        );
        // The route's limit counts the frames together, and refuses before the handler.
        let over = frames(vec![data("0123456789"), data("0123456789")]);
        assert_eq!(ask("POST", "/item/x", over).await.0, 413);
        let fits = frames(vec![data("01234567"), data("01234567")]);
        assert_eq!(
            ask("POST", "/item/x", fits).await,
            (200, "item x 16".into())
        );
        // A body that fails is a bad request.
        let broken = frames(vec![data("abc"), Err("connection reset")]);
        assert_eq!(ask("POST", "/echo", broken).await.0, 400);
        // Not a request: a method or a target that could not be one.
        assert_eq!(ask("GET", "/hello", frames(vec![])).await.0, 200);
        assert_eq!(
            ask("GET", "/q?s=%20a", frames(vec![])).await,
            (200, "s=%20a|5| a".into())
        );
    });
}

#[test]
fn the_host_and_the_peer_come_from_the_request() {
    run(async {
        let mut svc = wisp::tower::service::<Lab>().await.unwrap();
        // HTTP/2 names the host in the URI only; a `Host` header wins when there is one.
        let uri = Request::get("http://example.com:8080/host")
            .body(Body::full(""))
            .unwrap();
        let res = svc.call(uri).await.unwrap();
        assert_eq!(text(res.into_body()).await, "Some(\"example.com:8080\")");
        let both = Request::get("http://example.com/host")
            .header("host", "other.example")
            .body(Body::full(""))
            .unwrap();
        assert_eq!(
            text(svc.call(both).await.unwrap().into_body()).await,
            "Some(\"other.example\")"
        );
        let none = Request::get("/host").body(Body::full("")).unwrap();
        assert_eq!(
            text(svc.call(none).await.unwrap().into_body()).await,
            "None"
        );

        // The host puts the client's address in the extensions.
        let mut req = Request::get("/peer").body(Body::full("")).unwrap();
        req.extensions_mut()
            .insert("10.4.5.6:7".parse::<std::net::SocketAddr>().unwrap());
        assert_eq!(
            text(svc.call(req).await.unwrap().into_body()).await,
            "10.4.5.6:7|10.4.5.6"
        );
        let unknown = Request::get("/peer").body(Body::full("")).unwrap();
        assert_eq!(
            text(svc.call(unknown).await.unwrap().into_body()).await,
            "0.0.0.0:0|0.0.0.0"
        );
    });
}

#[test]
fn responses_carry_status_headers_and_bodies() {
    run(async {
        let mut svc = wisp::tower::service::<Lab>().await.unwrap();
        let page = svc
            .call(Request::get("/hello").body(Body::full("")).unwrap())
            .await
            .unwrap();
        assert_eq!(page.status(), 200);
        assert_eq!(page.headers()["content-type"], "text/html; charset=utf-8");
        assert_eq!(
            page.body().size_hint().exact(),
            Some(page.body().size_hint().lower())
        );
        assert!(text(page.into_body()).await.contains("<h1>hello</h1>"));

        // A header may repeat, and a value that cannot be sent is left out, not a panic.
        let cookies = svc
            .call(
                Request::get("/set-cookie?v=1")
                    .body(Body::full(""))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(cookies.headers().get_all("set-cookie").iter().count(), 1);
        let err = svc
            .call(Request::get("/err?k=teapot").body(Body::full("")).unwrap())
            .await
            .unwrap();
        assert_eq!(err.status(), 418);

        // HEAD keeps the length and drops the body.
        let head = svc
            .call(Request::head("/hello").body(Body::full("")).unwrap())
            .await
            .unwrap();
        assert!(head.headers().contains_key("content-length"));
        assert!(head.body().is_end_stream() || head.body().size_hint().exact() == Some(0));

        let stream = svc
            .call(Request::get("/stream").body(Body::full("")).unwrap())
            .await
            .unwrap();
        assert!(
            stream.body().size_hint().exact().is_none(),
            "a stream has no length"
        );
        assert!(!stream.body().is_end_stream());
        assert_eq!(text(stream.into_body()).await, "ab");
    });
}

#[test]
fn a_service_behind_the_app_answers_for_it() {
    run(async {
        let mut svc = wisp::tower::service::<Lab>().await.unwrap();
        let mut proxy = async |method: &str, target: &str| {
            let req = Request::builder()
                .method(method)
                .uri(target)
                .header("x-in", "seen")
                .body(Body::full("body"))
                .unwrap();
            let res: Response<Body> = svc.call(req).await.unwrap();
            let status = res.status().as_u16();
            let headers = res.headers().clone();
            (status, headers, text(res.into_body()).await)
        };
        // A body of known size is read whole; the request arrives as the client made it.
        let (status, headers, body) = proxy("POST", "/proxy/full?a=1").await;
        assert_eq!(
            (status, body.as_str()),
            (200, "POST /proxy/full?a=1 Some(\"seen\")")
        );
        assert_eq!(headers["content-type"], "text/x-behind");
        assert_eq!(headers["x-kept"], "yes");
        // What the app frames itself is not copied from behind.
        assert_ne!(
            headers.get("content-length").map(|v| v.to_str().unwrap()),
            Some("999")
        );
        assert_ne!(
            headers.get("date").map(|v| v.to_str().unwrap()),
            Some("never")
        );
        assert_eq!(
            headers.get_all("set-cookie").iter().count(),
            2,
            "every value of a repeated header"
        );
        // One whose size is not known is streamed as it comes.
        let (status, _, body) = proxy("GET", "/proxy/stream").await;
        assert_eq!(
            (status, body.as_str()),
            (201, "GET /proxy/stream Some(\"seen\")|two")
        );
        // Behind is down: the app's 500.
        assert_eq!(proxy("GET", "/proxy/fail").await.0, 500);
    });
}
