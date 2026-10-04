# Testing and mixing with other Rust code

The app is a function from request to reply.

## In process

```rust
wisp::prepare::<App>().await?;                        // runs `init`, once
let mut req = wisp::Request::new("GET", "/posts?page=2");
req.header("accept", "text/html");
let reply = wisp::handle::<App>(req).await;           // reply.status, .headers (Vec<(name, value)>), .body (wisp::Body)
```

Same parser, limits, hooks and CSRF check as the server.

## Testing an app

`wisp::test::client` needs no port and keeps cookies like a browser:

```rust
// tests/app.rs
wisp::app!();

#[test]
fn counter() {
    let mut app = wisp::test::client::<App>();
    assert!(app.get("/").text().contains("Clicked 0 times"));
    app.post_form("/?/increment", &[]);
    assert!(app.get("/").text().contains("Clicked 1 times"));
    assert!(app.cookie("user").is_none());
}
```

`get`, `post_form(target, &[(name, value)])`, `send(req)`,
`next_chunk(&mut reply)` (streams), `cookie(name)`; more in AGENTS.md and
api.md. Nothing upgrades in process, so `Response::websocket` is 501: test
WebSockets against the running server (`tests/app/tests/http.rs` uses a
`TcpStream`).

## WebSockets

A `+server.rs` upgrades with `Response::websocket`; the connection closes
when the handler returns:

```rust
// src/routes/ws/+server.rs
fn get() -> Response {
    Response::websocket(|ws| async move {
        while let Some(msg) = ws.recv().await {
            ws.send(msg).await?;          // a String is text, a Vec<u8> binary
        }
        Ok(())
    })
}
```

- `ws.recv()` is the next `wisp::Message` (`Text`/`Binary`; `msg.text()`,
  `msg.bytes()`), or `None` once the client closed or went, or the server
  stops. Pings are answered, fragments joined.
- `ws.send(msg)` fails once closed: stop. `recv` and `send` can overlap
  (`tokio::select!`, or `wisp::spawn` a sender with `ws.clone()`).
- While `recv` waits, a client quiet for 30 s is pinged and for 60 s closed
  (1001); `WISP_WS_IDLE` sets the 60 (seconds; `0` never). A send-only
  handler isn't timed (a client that stops reading fails `send`).
- A message is at most the route's `BODY_LIMIT` (1 MB default), else close
  1009. `before` runs first (cookies, hooks). A page on another site is 403
  as for a cross-site form (`Origin` must name the host, or `ORIGIN` when
  set); a non-upgrade request gets 426.
- The built-in server upgrades, and so do the `node`, `bun`, `deno`,
  `cloudflare` and `pages` builds (docs/deploy.md); `tower`, `vercel`,
  `netlify`, `lambda` and the test client answer 501.

Browser side: `new WebSocket(`${location.protocol === 'https:' ? 'wss' : 'ws'}://${location.host}/ws`)`
with `onmessage`, `onopen`, `onclose`.

## The `tower` feature

Wisp becomes a `tower::Service`; the default build keeps its two deps.

```toml
wisp = { git = "https://github.com/wyziedevs/wisp", features = ["tower"] }
```

```rust
let wisp = wisp::tower::service::<App>().await?;   // Service<http::Request<B>>
```

axum answers its own routes, Wisp the rest (`examples/axum`):

```rust
wisp::app!();

#[tokio::main]
async fn main() -> std::io::Result<()> {
    let listener = tokio::net::TcpListener::bind(wisp::address()).await?;
    let wisp = wisp::tower::service::<App>().await?;
    let app = axum::Router::new()
        .route("/api/hello", axum::routing::get(|| async { "hello from axum" }))
        .fallback_service(wisp);
    axum::serve(listener, app).await
}
```

Middleware: `tower::ServiceBuilder::new().layer(tower_http::compression::CompressionLayer::new()).service(wisp::tower::service::<App>().await?)`.

hyper:

```rust
let svc = hyper_util::service::TowerToHyperService::new(wisp::tower::service::<App>().await?);
hyper_util::server::conn::auto::Builder::new(hyper_util::rt::TokioExecutor::new())
    .serve_connection(hyper_util::rt::TokioIo::new(stream), svc)
    .await?;
```

Lambda with a `main` of your own (`--target lambda` needs no code;
`WISP_SECRET` in the function's environment):

```rust
wisp::app!();

#[tokio::main]
async fn main() -> Result<(), lambda_http::Error> {
    lambda_http::run(wisp::tower::service::<App>().await?).await
}
```

axum inside Wisp: send paths to an axum `Router` from `before` (or a
catch-all `+server.rs`). `before` may return `Result<Option<Response>>`
(`Some` answers instead of the route); `async fn before` takes every route
off the no-wait fast path:

```rust
// src/hooks.rs
async fn before(cx: &mut Cx) -> Result<Option<Response>> {
    if cx.path().starts_with("/api") {
        let mut api = api_router();                      // an axum::Router
        return Ok(Some(wisp::tower::call(&mut api, cx).await?));
    }
    Ok(None)
}
```
