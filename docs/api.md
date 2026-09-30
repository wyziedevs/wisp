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
    let new: NewNote = serde_json::from_slice(cx.body()).or_status(400)?;
    if new.title.is_empty() || new.title.len() > 200 {
        return error(422, "title must be 1 to 200 characters");
    }
    Ok(Response::json(serde_json::to_string(&db::add(new.title, new.tags))?).with_status(201))
}
```

## A resource

For most data the type is the whole API:

```rust
// src/routes/api/notes/+server.rs
#[derive(Rest)]
#[rest(write = "API_KEY")]
struct Note {
    #[validate(len = 1..=200)]
    title: String,
    done: bool,
}
```

| Request | Answer |
|---|---|
| `GET /api/notes` | the rows, in order of their ids: `[{"id":1,"title":"Tea","done":false}]` |
| `POST /api/notes` | 201 with the new row and its `Location`; 422 if the body does not pass; an array makes several, all or none |
| `GET /api/notes/1` | the row with its `etag`, or 404 |
| `PUT /api/notes/1` | the row replaced by the body |
| `PATCH /api/notes/1` | the body's members replace the row's (`null` empties an `Option`); the result must pass (422) |
| `DELETE /api/notes/1` | 204, or 404 |

`#[derive(Rest)]` is `Json` and `FromJson` (with the `#[validate]` rules
below) plus a table of its own, `Note::table()`, which pages and other
routes read too: `Note::table().all()`. A handler the file writes itself
(`fn delete(id: u64)`, say) answers in place of that one, and
`fn before(cx: &mut Cx)` runs before each.

`#[rest(...)]` takes:

| Setting | Does |
|---|---|
| `write = "API_KEY"` | writes answer 401 unless they bring `Authorization: Bearer <the value of API_KEY>` |
| `key = "KEY"` | the same for every request |
| `admin = "ADMIN_KEY"` | the same for DELETE |
| `table = "notes"` | its name in the store (the type's, lowercased, by default) |
| `ids = "random"` | ids no one can count through: random numbers below 2^53, which JavaScript reads exactly |
| `memory` | kept in memory only |

A field left out of a request is `false` for a `bool`, `[]` for a `Vec` and
`None` for an `Option`; others are required. Fields named `created_at` and
`updated_at` are Wisp's to set: when the row was made, and last changed, as
RFC 3339 text in a `String` (`2026-09-29T12:00:00Z`) or whole seconds since
1970 in a number.

### Queries

A list takes, in any mix:

| Query | Gives |
|---|---|
| `?done=true` | rows whose field is that (numbers compare as numbers) |
| `?points.gte=3` | `.ne`, `.gt`, `.gte`, `.lt`, `.lte`: compared; text and times (RFC 3339) compare as text |
| `?title.has=tea` | text that has it, in any case; a list (`tags.has=home`) that has the item |
| `?sort=-created_at,title` | sorted by fields, `-` for descending (`id` too) |
| `?limit=20&after=40` | 20 rows after id 40: a cursor that holds while rows are added |
| `?limit=20&offset=40` | 20 rows after the first 40 (for a sorted list) |
| `?fields=title,done` | only those fields, and `id` (a single row takes it too) |

A field that does not exist is a 400 that lists the ones that do. The
answer says how many rows pass the filters in `x-total-count`, and where
the next page is in `link: </api/notes?limit=20&after=60>; rel="next"`. With
`accept: application/x-ndjson` the rows come a line each.

### Caching and concurrent changes

Every row and list comes with an `etag`. A client that sends it back as
`if-none-match` gets a 304 with no body while the data is unchanged. A write
that sends `if-match` with the etag it read is refused with a 412 (`code`
`changed`) when someone else changed the row since, so no update is lost.

`const CACHE: u32 = 5;` in a `+server.rs` keeps each GET's answer (per
path and query) in each worker for 5 seconds and sends those bytes again,
for requests without a cookie or `authorization` (`CACHE_PUBLIC` for every
request). A write does not clear it: a read may be 5 seconds old. See
[design.md](design.md#page-logic) for the rules.

### Hooks

Plain functions in the same file, named for when they run:

```rust
fn before_create(note: &mut Note) -> Result {
    note.title = note.title.trim().to_string();
    if note.title == "admin" {
        return invalid("title", "is reserved");
    }
    Ok(())
}

fn after_update(note: &Row<Note>) {
    wisp::channel("notes").send(wisp::to_json(note));
}
```

`before_create` and `before_update` take the value (`&mut Note` to change
it, or `&Note`), and `before_update` its `id: u64` too; `before_delete`,
`after_create`, `after_update` and `after_delete` take the row
(`&Row<Note>`, or `&Note` and `id: u64`). Any may take `cx`, and return
nothing or a `Result`: a `before_` hook's error stops the change (a 422 from
`invalid` names the field), and an `after_` hook runs once the change is
saved. Hooks run outside the table's lock, so they may read the table.

### A resource under another

In `src/routes/users/[user=int]/notes/+server.rs`, a type with a field named
`user` holds each user's own: the route's `user` filters every request and
is set on every row made or changed there, whatever the body says.

### Where rows are kept

Rows are saved in the app's store before they change in memory, and read
back the first time the table is used, so they are there again after a
restart or a crash. By default the store is a log file per table
(`note.log`) in `WISP_DATA`: `.wisp/data` in the project in dev builds,
`data` in the working folder in release ones, `/data` in the Docker image
`wisp build --docker` writes (a volume). Each change is one appended line,
and a line a crash cut short is dropped when the log is read; a log grown to
twice its rows is written out again, whole, by the write that grew it.
`WISP_FSYNC` says when changes reach the disk itself: `second` (default),
`always` (before the answer), or `off`. `WISP_DATA=off` keeps every table
in memory, as the test client and edge builds do.

Any database can keep them instead, with a few lines in `src/hooks.rs`: a
store reads a table's rows and keeps each change, as JSON.

```rust
// src/hooks.rs, with rusqlite in Cargo.toml
struct Db(std::sync::Mutex<rusqlite::Connection>);

impl wisp::Store for Db {
    fn load(&self, table: &str) -> Result<Vec<(u64, String)>> {
        let db = self.0.lock().unwrap();
        db.execute(&format!("create table if not exists {table} (id integer primary key, json text)"), ())?;
        let mut rows = db.prepare(&format!("select id, json from {table}"))?;
        let rows = rows.query_map((), |r| Ok((r.get(0)?, r.get(1)?)))?;
        Ok(rows.collect::<Result<_, _>>()?)
    }
    fn save(&self, table: &str, id: u64, json: Option<&str>) -> Result {
        let db = self.0.lock().unwrap();
        match json {
            Some(j) => db.execute(&format!("insert or replace into {table} values (?1, ?2)"), (id, j))?,
            None => db.execute(&format!("delete from {table} where id = ?1"), (id,))?,
        };
        Ok(())
    }
}

fn init() -> Result {
    wisp::store(Db(std::sync::Mutex::new(rusqlite::Connection::open("app.db")?)));
    Ok(())
}
```

Redis (`HGETALL`/`HSET`/`HDEL` on a hash per table) and Postgres (a
`jsonb` column) take the same shape; the calls are blocking, one per change,
while the table is locked, which suits a database close by. Tables keep
every row in memory, so queries never wait on the store: for data larger
than memory, or queries a database should run, write the handlers (below).
On the edge (`wisp build --target cloudflare` and the like) each instance
has its own memory, so tables there are caches: keep data in D1 or KV
through `wisp::edge::fetch` in handlers of your own.

A table of a page's own is saved the same way:
`static TODOS: Table<Todo> = Table::saved("todos");`. A field added to a
saved type later must be an `Option`, a `Vec` or a `bool`, so the rows saved
before it still read.
## Handlers for a folder and its `[id]`

One file serves a collection and its members. A handler with a parameter
named `id`, in a folder whose URL has no `id`, answers at `/[id]` below it
(`[id=int]` when the `id` is a number); `list` is the folder's GET, since
`get` is then the member's:

```rust
// src/routes/api/notes/+server.rs
fn list() -> Vec<Note> { db::notes() }                        // GET /api/notes
fn post(body: New) -> Response { Response::created(&db::add(body)) }
fn get(id: u64) -> Option<Note> { db::note(id) }              // GET /api/notes/7
fn patch(id: u64, body: Changes) -> Option<Note> { db::change(id, body) }
fn delete(id: u64) -> Option<()> { db::remove(id) }           // 204, or 404
```

A folder with its own `[id]` (`api/notes/[id]/+server.rs`) still works, and
two files that would answer the same URLs are a build error.

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
`Option` field may be left out or `null`, and a `bool` one left out is
`false`; fields the struct does not have are ignored. A tuple struct with one field is read as that field, and an
enum without fields from its variant's name (`"Low"`). It is implemented
for strings, numbers (whole numbers must fit: `300` is not a `u8`), `bool`,
`Option`, `Vec`, `Box`, maps with string keys, and `wisp::Value`, which is
any JSON (`body: Value`, then `body.get("title")`).

Checks go on fields:

| Rule            | Checks                                   | On |
|-----------------|------------------------------------------|----|
| `len = 1..=200` | so many characters, or items (`1..`, `..=200` too) | strings, lists |
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
| `Option<T>`                  | the value as JSON, 204 for `Some(())`, or 404 for `None` |
| any of those in a `Result`   | the same, or the error |

`Response::created(&value)` is a 201 with the value. `Response::json_of`
sends any value with the status you give it (`.with_status(202)`).

## Errors are JSON

An error on a request under `/api`, or one that sent JSON, or one whose
`Accept` asks for JSON and not HTML, is answered as JSON instead of an error
page:

```json
{"status": 422, "code": "invalid", "error": "title: must have at least 1 character",
 "errors": {"title": "must have at least 1 character"}}
```

`errors` is there only for input that did not pass. `code` is for clients
to match on: the status's (`bad_request`, `unauthorized`, `forbidden`,
`not_found`, `method_not_allowed`, `conflict`, `precondition_failed`,
`too_large`, `unsupported_media_type`, `invalid`, `rate_limited`,
`internal`, `unavailable`...), or one of your own:
`Err(Error::new(409, "That email is taken").with_code("email_taken"))`.
A `+server.rs` answers every other client in JSON too, whatever its path;
only a browser page (whose `accept` has `text/html`) gets the error page.

A client whose `accept` asks for `application/problem+json`, or every
client with `WISP_PROBLEM_JSON=on`, gets RFC 9457's form instead:
`{"type":"about:blank","title":"Unprocessable Content","status":422,"code":"invalid","detail":"…","errors":{…}}`.

That covers every
error: a route that does not exist (404), a method the route does not take
(405, with `Allow`), `return error(403, "...")`, a panic (500; details only
in dev builds). A route is free to answer in its own format with a
`Response` instead.

OPTIONS on any route answers 204 with `Allow`, and a method the route does
not take answers 405 with the same `Allow`.

## Webhooks

A service that calls the app signs the body with a secret you share; check
it before trusting anything in it:

```rust
// src/routes/hooks/github/+server.rs
fn post(cx: &mut Cx, body: Value) -> Result {
    cx.need_signature("GITHUB_SECRET", "x-hub-signature-256")?; // 401 unless it matches
    // ...
    Ok(())
}
```

The signature is HMAC-SHA256 of the body with the secret in the variable,
in hex (with `sha256=` in front, as GitHub sends it, or without) or base64
(Shopify). Stripe's `stripe-signature` (`t=…,v1=…`) signs the time with the
body and is refused five minutes after. For any other scheme,
`wisp::hex(&wisp::hmac_sha256(secret, message))` and `wisp::secure_eq`.

## Idempotent retries

A client that sends a POST with an `Idempotency-Key` header may send it
again (after a timeout, say) and get the first answer back, marked
`idempotent-replayed: true`, instead of a second order or payment. Answers
are kept for a day per key, path and `authorization`; the same key with
another body is a 422, and one whose first request is still being answered
a 409. Nothing is kept for requests without the header.

## Big lists

`Response::ndjson` sends a list a JSON value per line, as it is made, so a
large export never sits whole in memory:

```rust
fn get() -> Response {
    Response::ndjson(|out| async move {
        for page in 0.. {
            let rows = db::page(page).await;
            if rows.is_empty() { break; }
            for row in rows { out.line(&row).await?; } // stops once the client has left
        }
        Ok(())
    })
}
```

## Versions and layout

Folders are URLs, so a version is a folder: `src/routes/api/v1/notes` and
`src/routes/api/v2/notes`. A `(group)` folder organizes files without
changing the URL. A `[id=int]` folder only matches numbers, so `/api/notes/abc`
is a 404 before any code runs. A header can pick a version too:
`let v: u32 = cx.header_or("x-api-version", 1);`.

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

- **API keys and tokens:** `cx.need_bearer("API_KEY")?` is a 401 unless
  the request has `Authorization: Bearer <the value of API_KEY>` (compared
  in constant time; an unset variable matches nothing). `cx.writes()` is
  true for POST, PUT, PATCH and DELETE, for keys that guard only writes.
  `cx.bearer()` is the token itself; compare secrets with
  `wisp::secure_eq(token, key)`.
- **Basic auth:** `cx.basic_auth()` is `(user, password)`.
- **Sessions:** `cx.set_signed_cookie("user", id)` when someone signs in, and
  `cx.signed_cookie("user")` on every request after; a visitor can read it
  but cannot forge it (see [design.md](design.md#cookies)).
- **Guards** go in `before`: in `src/hooks.rs` it runs before every route,
  in a `+server.rs` before each of its handlers. It can stop a request with
  an error, and hand what it found to the route with `cx.set`:

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

A `#[derive(Rest)]` type can keep its rows in any database through
`wisp::Store` ([above](#where-rows-are-kept)). For queries of your own, Wisp
brings no database layer, so any works. Open it once in `init`,
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

### A typed client

`/_wisp/client.ts` (served beside the docs), or the file `wisp build
--client ts [--out web/api.ts]` writes, is a TypeScript module made from
the same description, with no dependencies (it uses `fetch`): an interface
per type, and a method per endpoint whose arguments and result are typed.

```ts
import { client, WispError } from './client';

const api = client({ base: 'https://api.example.com', token: key });
const note = await api.postApiNotes({ title: 'Tea' }); // NoteInput: what may be left out is optional
const page = await api.getApiNotes({ sort: '-created_at', limit: 20 });
try {
  await api.deleteApiNotesId(note.id);
} catch (e) {
  if (e instanceof WispError && e.code === 'not_found') { /* already gone */ }
}
```

Method names are the operations': the method, then the path. A PATCH
takes a Partial of the type.

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

It also has `put_json`, `patch_json`, `delete`, `get`, `post_form`,
`send_json(method, url, json)`, `header(name, value)` for the next request
only (`if-match`, `idempotency-key`), `send(Request)` for anything else, and
`next_chunk` to read events as they are sent. Tables stay in memory in
tests, so each test process starts empty. The API template keeps its tests
in `src/tests.rs`, run by `cargo test`.

## Where it runs

Everything here works in the binary, Docker, Lambda and under the `tower`
feature. The edge build (`wisp build --target cloudflare` and the like)
runs each request in an instance that may be its own, so `wisp::channel`,
`wisp::every` and `RateLimit`, which keep state in the process, are not in
it, and WebSockets answer 501 there; use the host's queues, cron triggers
and rate limiting. Tables are in memory there, per instance. JSON,
validation, errors, CORS, auth, webhooks and the docs work everywhere.

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
  already written (`wisp new --api`), a resource from a type (as a
  scaffold, but with nothing generated to keep), and channels for live
  updates.
- From **PostgREST** and **JSON:API**: filters, sorting, field selection and
  pages from the query, with nothing written for them.
- From **Stripe**: error codes to match on, `Idempotency-Key`, and signed
  webhooks.
