# APIs and platforms

A Wisp app can be a website, a JSON API, or both in one project: an API is
a folder of `+server.rs` files, in the same style as pages, with the same
one-binary deploy. This document covers what an API needs: JSON in and out,
validation, errors, CORS, auth, rate limits, live updates, background jobs,
databases, an OpenAPI description and tests.

Start one with:

```sh
wisp new my-api --api
cd my-api
wisp dev          # then open http://127.0.0.1:3000/_wisp/docs
```

The template is [examples/api](../examples/api): a notes API with
validation, an API key, rate limits, live events, a WebSocket chat room and
tests.

## An endpoint

```rust
// src/routes/api/notes/+server.rs
#[derive(FromJson)]
struct NewNote {
    #[validate(min_len = 1, max_len = 200)]
    title: String,
    tags: Option<Vec<String>>,
}

fn get(q: Option<String>) -> Vec<Note> {
    db::notes(q.as_deref())
}

fn post(body: NewNote) -> Response {
    Response::created(&db::add(body.title, body.tags.unwrap_or_default()))
}
```

That is the whole file. The function's name is the method (`get`, `post`,
`put`, `patch`, `delete`); HEAD is answered by `get`, and OPTIONS by Wisp.

Before, the same endpoint took a `cx`, parsed the body itself and built its
own errors:

```rust
async fn post(cx: &mut Cx) -> Result<Response> {
    let new: NewNote = serde_json::from_slice(cx.body()).or_400()?;
    if new.title.is_empty() || new.title.len() > 200 {
        return error(422, "title must be 1 to 200 characters");
    }
    Ok(Response::json(serde_json::to_string(&db::add(new.title, new.tags))?).with_status(201))
}
```

## Input

Every parameter other than `cx` is read from the request by its name, as in
pages (see [design.md](design.md#page-logic)): a route parameter, then a
form field or a member of a JSON object body, then the query. The type says
how, and a value that is missing or not that type is an error that names it.
So `fn put(id: u64, title: String, done: bool)` takes a form post and
`{"title": "Tea", "done": true}` alike.

`body: T` is the whole JSON body read as a `T`:

| Sent                                       | Answer |
|--------------------------------------------|--------|
| JSON that is a `T` and passes its checks   | the handler runs |
| text that is not JSON                      | 400, saying where: `Invalid JSON: expected `:` at line 1, column 9` |
| JSON that is not a `T`, or fails a check   | 422, every problem by field |
| a `Content-Type` other than JSON           | 415 |
| no body, for `body: Option<T>`             | `None` |

(A `body` whose type is a string is still a form field called `body`.)

`#[derive(FromJson)]` reads a struct from an object of its fields. An
`Option` field may be left out or `null`; fields the struct does not have
are ignored. A tuple struct with one field is read as that field, and an
enum without fields from its variant's name (`"Low"`). It is implemented
for strings, numbers (whole numbers must fit: `300` is not a `u8`), `bool`,
`Option`, `Vec`, `Box`, maps with string keys, and `wisp::Value`, which is
any JSON (`body: Value`, then `body.get("title")`).

Checks go on fields:

| Rule            | Checks                                   | On |
|-----------------|------------------------------------------|----|
| `min = 0`       | at least                                 | numbers |
| `max = 100`     | at most                                  | numbers |
| `min_len = 1`   | at least so many characters, or items    | strings, lists |
| `max_len = 200` | at most so many                          | strings, lists |
| `email`         | one `@`, a dot in the domain, no spaces  | strings |

A field that is `None` passes; whether it may be left out is its type's
business. A check the derive does not know is a build error that lists the
ones it does.

Checks of your own (is this email taken?) return the same 422:
`return invalid("email", "is already taken")`. `Error::invalid(..).and(..)`
names more than one field.

`wisp::from_json::<T>(bytes)` reads JSON from anywhere, with the same
errors, and `wisp::json::parse` gives the `Value`. The parser is strict
RFC 8259 JSON, written in Wisp like the rest of the runtime.

## Output

| The function returns        | The client gets |
|-----------------------------|-----------------|
| a value (`#[derive(Json)]`, `Vec`, numbers, strings, maps...) | 200 and the value as JSON |
| nothing, or `Result<()>`     | 204 |
| a `Response`                 | that response |
| `Option<Response>`           | the response, or 404 for `None` |
| any of those in a `Result`   | the same, or the error |

`Response::created(&value)` is a 201 with the value. `Response::json_of`
sends any value with the status you give it (`.with_status(202)`).

## Errors are JSON

An error on a request under `/api`, or one that sent JSON, or one whose
`Accept` asks for JSON and not HTML, is answered as JSON instead of an error
page:

```json
{"status": 422, "error": "title: must have at least 1 character",
 "errors": {"title": "must have at least 1 character"}}
```

`errors` is there only for input that did not pass. That covers every
error: a route that does not exist (404), a method the route does not take
(405, with `Allow`), `return error(403, "...")`, a panic (500; details only
in dev builds). A route is free to answer in its own format with a
`Response` instead.

OPTIONS on any route answers 204 with `Allow`, and a method the route does
not take answers 405 with the same `Allow`.

## Versions and layout

Folders are URLs, so a version is a folder: `src/routes/api/v1/notes` and
`src/routes/api/v2/notes`. A `(group)` folder organizes files without
changing the URL. A `[id=int]` folder only matches numbers, so `/api/notes/abc`
is a 404 before any code runs.

## CORS

For pages on other sites to call the API from the browser:

```rust
// src/hooks.rs
fn before(cx: &mut Cx) -> Option<Response> {
    cx.cors("*")
}
```

`cx.cors` takes `*` for any site, or the sites allowed
(`"https://app.example.com https://example.com"`), which may then also send
cookies. It answers the browser's preflight itself, and a request from a
site that is not allowed gets no CORS headers, so its browser keeps the
answer from it. To allow it on some paths only:
`if cx.path().starts_with("/api/") { return cx.cors("*") }`.

## Auth

- **API keys and tokens:** `cx.bearer()` is the token of
  `Authorization: Bearer <token>`. Compare secrets with
  `wisp::secure_eq(token, key)`, which takes the same time wherever they
  differ.
- **Basic auth:** `cx.basic_auth()` is `(user, password)`.
- **Sessions:** `cx.set_signed_cookie("user", id)` when someone signs in, and
  `cx.signed_cookie("user")` on every request after; a visitor can read it
  but cannot forge it (see [design.md](design.md#cookies)).
- **Guards** go in `before`, which runs before every route. It can stop a
  request with an error, and hand what it found to the route with `cx.set`:

```rust
fn before(cx: &mut Cx) -> Result<()> {
    if cx.path().starts_with("/api/admin/") {
        let user = users::by_token(cx.bearer().unwrap_or("")).or_status(401)?;
        if !user.admin {
            return error(403, "Admins only");
        }
        cx.set(user); // a route reads it with cx.get::<User>()
    }
    Ok(())
}
```

## Rate limits

```rust
static LOGINS: RateLimit = RateLimit::per_minute(10);

fn post(cx: &mut Cx, name: String, password: String) -> Result<()> {
    LOGINS.check(cx.client_ip())?; // a 429 with retry-after once the ten are used
    ...
}
```

A key may be anything hashable: the client's IP, an API key, a user id. It
may use all its requests at once, then gets them back steadily over the
window. Behind a proxy, set `WISP_CLIENT_IP_HEADER` so `cx.client_ip()` is
the client's and not the proxy's. `BODY_LIMIT` caps request bodies per route
(see [design.md](design.md#page-logic)).

## Live updates

A channel carries messages from one request to every other that listens:

```rust
// Anywhere: a note was added.
wisp::channel("notes").send(wisp::to_json(&note));

// src/routes/api/events/+server.rs: each one, as server-sent events.
fn get() -> Response {
    let mut notes = wisp::channel("notes").subscribe();
    Response::events(|events| async move {
        while let Some(note) = notes.recv().await {
            events.event(&note).await?;
        }
        Ok(())
    })
}
```

A WebSocket joins a channel both ways with `connect`: a chat room is

```rust
fn get() -> Response {
    Response::websocket(|ws| async move {
        wisp::channel("chat").connect(&ws).await?;
        Ok(())
    })
}
```

Channels are in the process. Several servers of one app each have their
own; relay between them through Redis pub/sub or Postgres `LISTEN` in a task
started from `init`.

## Background jobs

```rust
// src/hooks.rs
fn init() {
    wisp::every(Duration::from_secs(3600), || async {
        db::delete_expired_sessions().await;
    });
}
```

`wisp::every` runs until the server stops, never two at once.
`wisp::spawn` runs one task. For work that must survive a restart, keep a
queue in the database and have `every` take from it.

## Health checks

A health check is a route like any other:

```rust
// src/routes/healthz/+server.rs
fn get() -> &'static str {
    "ok"
}
```

Add a database ping to it if the load balancer should take a server out
when its database is gone.

## Configuration

`wisp::env("KEY")` reads a variable (the process's on a server, the
worker's on the edge), and `wisp::env_or("WORKERS", 4)` parses one, with a
default. Read settings once in `init` and share them:

```rust
pub struct Config {
    pub api_key: String,
    pub workers: usize,
}

fn init() {
    wisp::provide(Config {
        api_key: wisp::env("API_KEY").expect("set API_KEY"),
        workers: wisp::env_or("WORKERS", 4),
    });
}

// Anywhere after: wisp::state::<Config>().api_key
```

## Databases

Wisp brings no database layer, so any works. Open it once in `init`,
`wisp::provide` it, and read it with `wisp::state` in routes. With
[sqlx](https://crates.io/crates/sqlx) and Postgres:

```rust
// src/hooks.rs
async fn init() -> Result<()> {
    let url = wisp::env("DATABASE_URL").expect("set DATABASE_URL");
    wisp::provide(sqlx::PgPool::connect(&url).await?);
    Ok(())
}

// src/routes/api/notes/[id=int]/+server.rs
async fn get(id: i64) -> Result<Note> {
    let db = wisp::state::<sqlx::PgPool>();
    let row = sqlx::query_as!(Note, "select id, title from notes where id = $1", id)
        .fetch_optional(db)
        .await?;
    row.or_404()
}
```

[rusqlite](https://crates.io/crates/rusqlite) is not async: keep the
connection in a `Mutex` and hold it briefly (or run long queries in
`tokio::task::spawn_blocking`):

```rust
wisp::provide(std::sync::Mutex::new(rusqlite::Connection::open("app.db")?));

let db = wisp::state::<std::sync::Mutex<rusqlite::Connection>>().lock().unwrap();
```

[redis](https://crates.io/crates/redis), for a cache or to relay channels
between servers: `wisp::provide(redis::Client::open(url)?)`, then
`wisp::state::<redis::Client>().get_multiplexed_async_connection().await?`.

All of these need tokio, so they run in the binary, Docker and Lambda, not
on the edge; there, use a database over HTTP through `wisp::edge::fetch`
(see [deploy.md](deploy.md#what-works-on-the-edge)).

## OpenAPI and the docs page

The build describes every `+server.rs` endpoint in an OpenAPI 3.1 document,
served at `/_wisp/openapi.json`, and `/_wisp/docs` lists them with a form to
send each one a request (with a bearer token, if you give one). Both are on
in dev builds and off in release ones; `WISP_API_DOCS=on` serves them in
release too, and `off` hides them in dev.

It is made from what the build already reads, so there is nothing to
annotate: paths and their parameters, the query or form inputs of each
method by name and type, `body: T`, and what each returns. Types defined in
the route's `+server.rs` or in a module file in `src/` (`src/notes.rs`) are
described field by field. A type it cannot see (from another crate, or a
nested module) is named and left open rather than guessed, and checks such
as `min_len` are not in the document.

Any tool that reads OpenAPI takes it from there: client generators,
Postman, API gateways.

## Tests

The test client answers in process, with no port, through the same
routing, hooks and errors as the server:

```rust
#[test]
fn notes() {
    let mut app = wisp::test::client::<App>();
    app.bearer("dev-key"); // every request from now on

    let made = app.post_json("/api/notes", r#"{"title": "Buy tea"}"#);
    assert_eq!(made.status, 201);
    let note: Note = made.json(); // any FromJson type; Value for any JSON

    let bad = app.post_json("/api/notes", r#"{"title": ""}"#);
    assert_eq!(bad.status, 422);
}
```

It also has `put_json`, `patch_json`, `delete`, `get`, `post_form`, and
`next_chunk` to read events as they are sent. The API template keeps its
tests in `src/tests.rs`, run by `cargo test`.

## Where it runs

Everything here works in the binary, Docker, Lambda and under the `tower`
feature. The edge build (`wisp build --target cloudflare` and the like)
runs each request in an instance that may be its own, so `wisp::channel`,
`wisp::every` and `RateLimit`, which keep state in the process, are not in
it, and WebSockets answer 501 there; use the host's queues, cron triggers
and rate limiting. JSON, validation, errors, CORS, auth and the docs work
everywhere.

## Logging and request ids

Dev builds log every request with its time; release builds log what fails.
To pass a proxy's request id back:

```rust
fn before(cx: &mut Cx) {
    if let Some(id) = cx.header("x-request-id").map(str::to_string) {
        cx.set_header("x-request-id", id);
    }
}
```

## What it borrows

- From **FastAPI**: a typed body is validated before the handler runs, and
  every problem comes back at once, by field, as a 422. The OpenAPI
  document and a page to try the API come from the code, with nothing to
  annotate.
- From **Axum**: handlers are plain functions whose arguments say what they
  read, checked by the compiler; the app is a `tower::Service` when it needs
  to be ([embed.md](embed.md)).
- From **Next.js route handlers** and **Nuxt/Nitro**: a file per route, a
  function per method, `/api` beside the pages in one project.
- From **Hono**: CORS and auth as one line in the one hook, and the same app
  on a server or the edge.
- From **Rails** and **Phoenix**: a template to start from with tests
  already written (`wisp new --api`), and channels for live updates.
