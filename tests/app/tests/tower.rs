//! The test app as a tower service, as axum, hyper or Lambda would call it.
#![cfg(feature = "tower")]

use std::future::poll_fn;
use std::pin::pin;
use wisp::tower::http_body::Body as _;
use wisp::tower::tower_service::Service as _;
use wisp::tower::{Body, http};
use wisp_test_app::Site;

fn run<T>(f: impl Future<Output = T>) -> T {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(f)
}

async fn text(body: Body) -> String {
    let mut body = pin!(body);
    let mut all = Vec::new();
    while let Some(frame) = poll_fn(|cx| body.as_mut().poll_frame(cx)).await {
        all.extend_from_slice(&frame.unwrap().into_data().unwrap());
    }
    String::from_utf8(all).unwrap()
}

#[test]
fn serves_pages_forms_and_limits() {
    run(async {
        let mut svc = wisp::tower::service::<Site>().await.unwrap();

        let home = svc
            .call(http::Request::get("/").body(Body::full("")).unwrap())
            .await
            .unwrap();
        assert_eq!(home.status(), 200);
        assert_eq!(home.headers()["x-app"], "test");
        assert!(home.body().size_hint().exact().is_some());
        assert!(
            text(home.into_body())
                .await
                .contains("<h1>hello from init</h1>")
        );

        // HTTP/2 names the host in the URI: a same-origin form still passes.
        let login = http::Request::post("https://example.com/login")
            .header("origin", "https://example.com")
            .header("content-type", "application/x-www-form-urlencoded")
            .body(Body::full("name=ada"))
            .unwrap();
        let login = svc.call(login).await.unwrap();
        assert_eq!(
            (
                login.status().as_u16(),
                login.headers()["location"].to_str().unwrap()
            ),
            (303, "/admin")
        );

        let big = svc
            .call(
                http::Request::post("/echo")
                    .body(Body::full(vec![b'a'; 100 * 1024]))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(big.status(), 413);
        let echo = svc
            .call(http::Request::post("/echo").body(Body::full("hi")).unwrap())
            .await
            .unwrap();
        assert_eq!(text(echo.into_body()).await, "2:hi");

        let events = svc
            .call(http::Request::get("/events").body(Body::full("")).unwrap())
            .await
            .unwrap();
        assert!(
            text(events.into_body())
                .await
                .ends_with("data: tick 2\ndata: line two\n\n")
        );
    });
}

/// A tower service called from inside Wisp, as a hook would call an axum `Router`.
#[test]
fn calls_a_tower_service() {
    run(async {
        let mut wisp = wisp::tower::service::<Site>().await.unwrap();
        let cx = wisp::Cx::from_request::<Site>(
            "POST",
            "/echo?x=1",
            [("host", &b"x"[..])],
            b"hey",
            "127.0.0.1:1".parse().unwrap(),
        )
        .unwrap();
        let res = wisp::tower::call(&mut wisp, &cx).await.unwrap();
        assert_eq!(
            (res.status, &*res.content_type, res.body.as_slice()),
            (200, "text/plain; charset=utf-8", &b"3:hey"[..])
        );
        assert!(res.headers.iter().any(|(n, v)| n == "x-app" && v == "test"));
    });
}
