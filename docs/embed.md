# Testing and mixing with other Rust code

Wisp's server is one front end. The app itself is a function from a request
to a reply, and you can call it yourself.

## In process: `wisp::handle`

```rust
wisp::prepare::<App>().await?;                        // runs `init`, once

let mut req = wisp::Request::new("GET", "/posts?page=2");
req.header("accept", "text/html");
let reply = wisp::handle::<App>(req).await;           // wisp::Reply

reply.status;                                         // 200
reply.headers;                                        // Vec<(name, value)>
reply.body;                                           // wisp::Body
```

Requests go through the same parser, limits, hooks and CSRF check as the
built-in server's.

## Testing an app

`wisp::test::client` needs no port and no server. It keeps cookies between
requests, like a browser.

```rust
// tests/app.rs
wisp::app!();

#[test]
fn counter() {
    let mut app = wisp::test::client::<App>();

    let page = app.get("/");
    assert_eq!(page.status, 200);
    assert!(page.text().contains("Clicked 0 times"));

    app.post_form("/?/increment", &[]);
    assert!(app.get("/").text().contains("Clicked 1 times"));
}

#[test]
fn login_sets_a_cookie() {
    let mut app = wisp::test::client::<App>();
    app.post_form("/login", &[("name", "ada")]);
    assert!(app.cookie("user").is_some());
}
```

- `get(target)`, `post_form(target, &[(name, value)])`, and `send(req)` for
  anything else.
- `next_chunk(&mut reply)` reads a streamed reply one chunk at a time.
- `cookie(name)` is a cookie the client holds.

In process there is no connection to upgrade, so a `Response::websocket`
answers 501; test WebSockets against the running server (the test app's
`tests/http.rs` does it with a `TcpStream`).

## WebSockets

A `+server.rs` upgrades with `Response::websocket`. The handler gets the
socket and the connection closes when it returns:

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

- `ws.recv()` is the next `wisp::Message` (`Text` or `Binary`; `msg.text()`
  and `msg.bytes()` read either), or `None` once the client closed or went,
  or the server is stopping. Pings are answered and fragments put together
  for you.
- `ws.send(msg)` fails once the connection is closed: stop then. `recv` and
  `send` can run at once (`tokio::select!`, or `wisp::spawn` a sender with
  `ws.clone()`, the same socket).
- While `recv` waits, a client quiet for 30 seconds is pinged, and one
  quiet for 60 is closed (1001), so dead connections do not pile up.
  `WISP_WS_IDLE` sets the 60 (in seconds; `0` never closes). A handler that
  only sends is not timed: a client that stops reading fails its `send`.
- A message is at most the route's `BODY_LIMIT` (1 MB by default); a larger
  one closes the connection with code 1009.
- The request goes through `before` first, so its cookies and hooks apply.
  A page on another site is refused (403) as a cross-site form post is: the
  `Origin` must name the host, or `ORIGIN` when set. A request that is not
  an upgrade gets 426.
- Only the built-in server upgrades. The `tower` feature, the edge targets
  and `wisp::test::client` answer 501.

In the page, the browser's own `WebSocket` is all it takes:

```html
<script>
  const ws = new WebSocket(`${location.protocol === 'https:' ? 'wss' : 'ws'}://${location.host}/ws`);
  ws.onmessage = (e) => console.log('got', e.data);
  ws.onopen = () => ws.send('hello');
  ws.onclose = () => setTimeout(() => location.reload(), 1000); // or reconnect
</script>
```

## The `tower` feature

Turn it on and Wisp is a `tower::Service`. The default build is unchanged
and keeps its two dependencies.

```toml
wisp = { git = "https://github.com/wyziedevs/wisp", features = ["tower"] }
```

```rust
let wisp = wisp::tower::service::<App>().await?;   // Service<http::Request<B>>
```

### Wisp inside axum

axum answers its own routes, Wisp answers the rest.

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

A runnable version is in `examples/axum`.

### Tower middleware around Wisp

```rust
let wisp = tower::ServiceBuilder::new()
    .layer(tower_http::compression::CompressionLayer::new())
    .service(wisp::tower::service::<App>().await?);
```

Serve it with hyper or axum.

### Wisp on hyper

```rust
let wisp = wisp::tower::service::<App>().await?;
let svc = hyper_util::service::TowerToHyperService::new(wisp);
hyper_util::server::conn::auto::Builder::new(hyper_util::rt::TokioExecutor::new())
    .serve_connection(hyper_util::rt::TokioIo::new(stream), svc)
    .await?;
```

### Wisp on AWS Lambda

`lambda_http` takes a tower service:

```rust
wisp::app!();

#[tokio::main]
async fn main() -> Result<(), lambda_http::Error> {
    lambda_http::run(wisp::tower::service::<App>().await?).await
}
```

Set `WISP_SECRET` in the function's environment.

### axum inside Wisp

Send some paths to an axum `Router` from `before`, or from a catch-all
`+server.rs`:

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

`call` sends the request in `cx` to the service and returns its response.
