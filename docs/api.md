# APIs and platforms

An API is a folder of `+server.rs` files beside pages, one binary. Start
one: `wisp new my-api --api`, `wisp dev`, then `/_wisp/docs`. Template:
[examples/api](../examples/api).

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

The fn name is the method (`get post put patch delete`); HEAD is `get`,
OPTIONS is Wisp's (204 + `Allow`; a method the route lacks is 405 + `Allow`).
A handler may also take `cx: &mut Cx` and return `Result<Response>`.

## A resource

```rust
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
| `GET /api/notes` | rows by id: `[{"id":1,"title":"Tea","done":false}]` |
| `POST /api/notes` | 201 + row + `Location`; 422 if invalid; an array makes several, all or none |
| `GET /api/notes/1` | row with `etag`, or 404 |
| `PUT /api/notes/1` | replaced by the body |
| `PATCH /api/notes/1` | body's members replace the row's (`null` empties an `Option`); result must pass (422) |
| `DELETE /api/notes/1` | 204 or 404 |

`Rest` = `Json` + `FromJson` + a table `Note::table()` other routes read too.
A handler the file writes (`fn delete(id: u64)`) replaces that one;
`fn before(cx: &mut Cx)` runs before each.

| `#[rest(...)]` | Does |
|---|---|
| `write = "API_KEY"` | writes 401 unless `Authorization: Bearer <value of API_KEY>` |
| `key = "KEY"` | same for every request |
| `admin = "ADMIN_KEY"` | same for DELETE |
| `table = "notes"` | store name (default: type name lowercased) |
| `ids = "random"` | uncountable random ids below 2^53 |
| `memory` | memory only |

A field left out of a request: `bool` false, `Vec` `[]`, `Option` None;
others required. `created_at`/`updated_at` are set by Wisp (RFC 3339
`String`, or whole seconds in a number).

### Queries

| Query | Gives |
|---|---|
| `?done=true` | rows with that field value (numbers compare as numbers) |
| `?points.gte=3` | `.ne .gt .gte .lt .lte`; text and RFC 3339 times compare as text |
| `?title.has=tea` | text containing it (any case); a list containing the item |
| `?sort=-created_at,title` | `-` descending (`id` too) |
| `?limit=20&after=40` | 20 rows after id 40 (stable cursor) |
| `?limit=20&offset=40` | skip 40 (for a sorted list) |
| `?fields=title,done` | only those and `id` (a single row too) |

An unknown field is a 400 listing the known ones. `x-total-count` has the
match count; `link: <…?limit=20&after=60>; rel="next"` the next page;
`accept: application/x-ndjson` gives a line per row.

Every row and list has an `etag`; `if-none-match` gets a 304. A write with
`if-match` is a 412 (`code` `changed`) if the row changed since. `const
CACHE: u32 = 5;` in a `+server.rs` caches each GET per path and query per
worker (rules: design.md); a write does not clear it.

### Hooks

```rust
fn before_create(note: &mut Note) -> Result {
    note.title = note.title.trim().to_string();
    if note.title == "admin" { return invalid("title", "is reserved"); }
    Ok(())
}
fn after_update(note: &Row<Note>) {
    wisp::channel("notes").send(wisp::to_json(note));
}
```

`before_create`/`before_update` take the value (`&mut Note` or `&Note`;
update also `id: u64`); `before_delete`, `after_create`, `after_update`,
`after_delete` take the row (`&Row<Note>`, or `&Note` and `id: u64`). Any
may take `cx` and return nothing or `Result`; a `before_` error stops the
change (422 from `invalid` names the field); `after_` runs once saved. Hooks
run outside the table lock.

A type with a field named `user` in `users/[user=int]/notes/+server.rs`
holds each user's own: the route's `user` filters every request and is set
on every row made or changed there.

### Where rows are kept

Saved before memory changes, read at first use: a log file per table
(`note.log`, a line per change, a cut line dropped on read, rewritten whole
at twice its rows) in `WISP_DATA`: `.wisp/data` in dev, `data` in release,
`/data` in the `--docker` image. `WISP_FSYNC`: `second` (default), `always`,
`off`. `WISP_DATA=off`: memory (tests, edge). Tables hold every row in
memory; for larger data or database-side queries write the handlers.

Any database via `wisp::store(Db)` in `init`: implement `wisp::Store` with
`load(&self, table) -> Result<Vec<(u64, String)>>` (id and JSON of each row)
and `save(&self, table, id, json: Option<&str>) -> Result` (`None` deletes),
e.g. over `Mutex<rusqlite::Connection>` (`create table if not exists {table}
(id integer primary key, json text)`; `insert or replace`; `delete`). Calls
are blocking, one per change, under the table lock (suits a nearby
database). A POST of an array goes through `save_many(table, &[(id, json)])`
(default: each in turn, undoing earlier ones on failure; override with one
transaction); failure is a 500 and nothing is kept. On the edge each
instance has its own memory: tables are caches; use D1/KV through
`wisp::edge::fetch`, or env `WISP_STORE=d1:DB|deno-kv|libsql://…`.

A page's own table: `static TODOS: Table<Todo> = Table::saved();` (named
from the static; `Table::saved("todos")` names it). A field added to a saved
type must be `Option`, `Vec` or `bool`. An `Image` field is a `data:` URL.

`POSTS.page(cx, 10)` is the rows `?page=N` asks for, newest first; it reads
as its rows and has `number`, `prev`, `next` (hrefs, `None` at the ends):

```html
{#each posts as post}<p>{post.title}</p>{/each}
{#if let Some(href) = posts.next}<a {href}>Older</a>{/if}
```

## Handlers for a folder and its `[id]`

A handler with a param `id` in a folder whose URL has none answers at
`/[id]` below it (`[id=int]` when numeric); `list` is the folder's GET:

```rust
// src/routes/api/notes/+server.rs
fn list() -> Vec<Note> { db::notes() }                        // GET /api/notes
fn post(body: New) -> Response { Response::created(&db::add(body)) }
fn get(id: u64) -> Option<Note> { db::note(id) }              // GET /api/notes/7
fn patch(id: u64, body: Changes) -> Option<Note> { db::change(id, body) }
fn delete(id: u64) -> Option<()> { db::remove(id) }           // 204, or 404
```

A folder with its own `[id]` still works; two files answering the same URLs
is a build error.

## Input

Each param but `cx` is read by name: route param, then form field or JSON
object member, then query. `fn put(id: u64, title: String, done: bool)`
takes a form post and `{"title":"Tea","done":true}` alike. Missing or
wrong type is an error naming it. `body: T` is the whole JSON body:

| Sent | Answer |
|---|---|
| JSON that is a `T` and passes | handler runs |
| not JSON | 400 with where: `Invalid JSON: expected `:` at line 1, column 9` |
| JSON not a `T`, or failing a check | 422, every problem by field |
| non-JSON `Content-Type` | 415 |
| no body, `body: Option<T>` | `None` |

By-name params from a JSON body answer the same. (A `body: String` is a form
field called `body`.)

`#[derive(FromJson)]` reads a struct from an object: `Option` may be absent
or `null`, `bool` absent is false, extra members ignored; a one-field tuple
struct reads as that field; a fieldless enum from its variant name
(`"Low"`). Implemented for strings, numbers (`300` is not a `u8`), `bool`,
`Option`, `Vec`, `Box`, string-key maps and `wisp::Value` (any JSON:
`body.get("title")`).

| Rule | Checks | On |
|---|---|---|
| `len = 1..=200` | length or item count (`1..`, `..=200`) | strings, lists |
| `min = 0` `max = 100` | bounds | numbers |
| `min_len = 1` `max_len = 200` | length or items | strings, lists |
| `email` | what `<input type="email">` takes (`a@b` too) | strings |

`None` passes. Further rules (`url one_of pattern with`: docs/data.md).
An unknown rule is a build error listing the valid ones. Own checks:
`return invalid("email", "is already taken")` (422);
`Error::invalid(..).and(..)` names several fields. `wisp::from_json::<T>(bytes)`
reads JSON anywhere with the same errors; `wisp::json::parse` gives a
`Value` (strict RFC 8259).

## Output

| Returns | Client gets |
|---|---|
| a `Json` value (`#[derive(Json)]`, `Vec`, numbers, strings, maps) | 200 + JSON |
| nothing, `Result<()>` | 204 |
| `Response` | it |
| `Option<Response>` | it, or 404 |
| `Option<T>` | JSON, 204 for `Some(())`, 404 for `None` |
| any of these in a `Result` | same, or the error |

`Response::created(&v)` is 201; `Response::json_of(&v).with_status(202)`.

## Errors are JSON

An error is JSON (else the app's `+error.wisp`, else Wisp's default page)
when the request targets a `+server.rs` (or an unmatched path under a first
segment with endpoints and no pages), is under `/api`, sent JSON, prefers
JSON by `Accept`, or has no `Accept` and is not a browser navigating; an app
with no pages always answers JSON:

```json
{"status": 422, "code": "invalid", "error": "title: must have at least 1 character",
 "errors": {"title": "must have at least 1 character"}}
```

`errors` only for invalid input. `code`: the status's (`bad_request
unauthorized forbidden not_found method_not_allowed conflict
precondition_failed too_large unsupported_media_type invalid rate_limited
internal unavailable`) or your own: `Error::new(409, "That email is
taken").with_code("email_taken")`. `error` is your message, else the status
name. `accept: application/problem+json`, or `WISP_PROBLEM_JSON=on`, gives
RFC 9457 (`type`, `title`, `status`, `code`, `detail`, `errors`). Covers 404,
405, `error(403, "…")` and panics (500; details in dev only).

## Webhooks

```rust
// src/routes/hooks/github/+server.rs
fn post(cx: &mut Cx, body: Value) -> Result {
    cx.need_signature("GITHUB_SECRET", "x-hub-signature-256")?; // 401 unless it matches
    Ok(())
}
```

HMAC-SHA256 of the body with the secret in that variable, hex (with or
without `sha256=`) or base64 (Shopify). Stripe's `stripe-signature`
(`t=…,v1=…`) signs the time too and is refused after five minutes. Other
schemes: `wisp::hex(&wisp::hmac_sha256(secret, message))`, `wisp::secure_eq`.

## Idempotent retries

A POST with `Idempotency-Key` retried gets the first answer back with
`idempotent-replayed: true`, kept a day per key, path and `authorization`;
the same key with another body is 422, one still in progress 409. No header,
nothing kept.

## Big lists

```rust
fn get() -> Response {
    Response::ndjson(|out| async move {
        for page in 0.. {
            let rows = db::page(page).await;
            if rows.is_empty() { break; }
            for row in rows { out.line(&row).await?; } // stops once the client left
        }
        Ok(())
    })
}
```

## Versions, CORS

Versions are folders (`api/v1/notes`); `(group)` folders don't change URLs;
`[id=int]` 404s `/api/notes/abc` before any code runs; or
`cx.header_or("x-api-version", 1)`. `cx.cors("*")?` in `before` (a preflight
is the `Err` it returns; or `const CORS: &str = "*";`), or the allowed
sites `"https://app.example.com https://example.com"` (which may then send
cookies); a site not allowed gets no CORS headers. Some paths only:
`if cx.path().starts_with("/api/") { cx.cors("*")?; }`.

## Auth

- Keys: `cx.need_bearer("API_KEY")?` (401 unless `Authorization: Bearer
  <API_KEY's value>`, constant time, unset matches nothing); `cx.writes()` is
  true for POST/PUT/PATCH/DELETE; `cx.bearer()` is the token;
  `wisp::secure_eq(a, b)`. `cx.basic_auth()` is `Option<(user, password)>`.
- Sessions: `cx.sign_in(id)` after `wisp::password::check(&typed,
  user_hash).await?` (`None` for no such user, as slow; `hash(&password)
  .await?`; both on hashing threads), then `cx.signed_in()?` or
  `cx.user(&USERS)?` (`cx.login`/`signup`: docs/auth.md) where only members
  go: signed out, a page 303s to `/login` (`wisp::sign_in_page("/x")`), an
  endpoint or JSON client gets 401 (`signed_out`). `cx.sign_out()` ends it
  here. The id is in a signed cookie for 30 days; each `sign_in` sets a new
  session.
- `wisp::sign_out_everywhere(id)?` ends all earlier sessions of `id` on every
  device (counted in the saved table `wisp_sign_outs`; nothing is looked up
  until an app calls it; another instance sharing the store sees it on its
  next start; a store failure is its `Err`).
- Guards go in `before` (hooks.rs: every route; `+server.rs`: each
  handler); hand data on with `cx.set`:

```rust
fn before(cx: &mut Cx) -> Result<()> {
    if cx.path().starts_with("/api/admin/") {
        let user = users::by_token(cx.bearer().unwrap_or("")).or_status(401)?;
        if !user.admin { return error(403, "Admins only"); }
        cx.set(user); // a route reads it with cx.get::<User>()
    }
    Ok(())
}
```

## Rate limits

```rust
static LOGINS: RateLimit = RateLimit::per_minute(10);

fn post(cx: &mut Cx, name: String, password: String) -> Result<()> {
    LOGINS.check(cx.client_ip())?; // 429 + retry-after once the ten are used
    ...
}
```

Key: anything hashable (IP, API key, user id). It may spend all at once,
then refills steadily. Behind a proxy set `WISP_CLIENT_IP_HEADER`.
`BODY_LIMIT` caps bodies per route.

## Live updates

```rust
wisp::channel("notes").send(wisp::to_json(&note));     // anywhere

fn get() -> Response { wisp::channel("notes").events() }       // SSE
fn get() -> Response { wisp::channel("chat").websocket() }     // both ways
```

`subscribe()` (`recv().await`) feeds your own `Response::events` or
`websocket`; `connect(&ws)` joins an existing socket. Channels are per
process: relay between servers (Redis pub/sub, Postgres `LISTEN`) from a task
started in `init`.

## Background jobs

```rust
// src/hooks.rs
fn init() {
    wisp::every(Duration::from_secs(3600), || async { db::delete_expired_sessions().await; });
}
```

`every` runs until stop, never two at once; a panicking run is reported and
the next starts on time. `wisp::spawn` runs one task. Durable queues, cron:
docs/data.md. A health check is a route: `fn get() -> &'static str { "ok" }`
(`/_wisp/health` exists).

## Configuration and databases

`wisp::env("KEY")` (process env, else `.env` in the working directory, read
once at start; the process wins; bad lines skipped with a warning; edge: the
worker's env, no `.env`); `wisp::env_or("WORKERS", 4)` parses with a
default. Read settings once in `init`: `wisp::provide(v)`, then
`wisp::state::<T>()` (or `#[derive(Config)]`, AGENTS.md).

No database layer: open in `init`, `provide`, read with `state`
(`wisp::provide(sqlx::PgPool::connect(&url).await?)` in `async fn init`;
`wisp::state::<sqlx::PgPool>()` in a handler). rusqlite is sync: provide a
`Mutex<Connection>`, hold the lock briefly or `spawn_blocking`. redis:
`provide(redis::Client::open(url)?)`. These need tokio: binary, Docker,
Lambda, not the edge (use HTTP databases through `wisp::edge::fetch`).

## OpenAPI and docs page

The build describes every `+server.rs` endpoint as OpenAPI 3.1 at
`/_wisp/openapi.json`; `/_wisp/docs` lists them with a try-it form. On in
dev, off in release; `WISP_API_DOCS=on|off` overrides. Nothing to annotate
(paths, params, query/form inputs by name and type, `body: T`, returns).
Types from the route file or `src/*.rs` are described by field; others are
named and left open; checks like `min_len` are not included.

Typed TypeScript client from the same description, no dependencies:
`/_wisp/client.ts`, or `wisp build --client ts [--out web/api.ts]`:

```ts
import { client, WispError } from './client';
const api = client({ base: 'https://api.example.com', token: key });
const note = await api.postApiNotes({ title: 'Tea' });
const page = await api.getApiNotes({ sort: '-created_at', limit: 20 });
try { await api.deleteApiNotesId(note.id); }
catch (e) { if (e instanceof WispError && e.code === 'not_found') {} }
```

Method name = HTTP method + path; PATCH takes a Partial; fields that may
be left out are optional.

## Tests

In process, no port, same routing, hooks and errors:

```rust
#[test]
fn notes() {
    let mut app = wisp::test::client::<App>();
    app.bearer("dev-key"); // every request from now on
    let made = app.post_json("/api/notes", r#"{"title": "Buy tea"}"#);
    assert_eq!(made.status, 201);
    let note: Note = made.json(); // any FromJson type; Value for any JSON
    assert_eq!(app.post_json("/api/notes", r#"{"title": ""}"#).status, 422);
}
```

Also `get put_json patch_json delete post_form send_json(method, url, json)`,
`header(name, value)` (next request only: `if-match`, `idempotency-key`),
`send(Request)`, `next_chunk` (events as sent). Tables are in memory.

### In a browser

With feature `browser` (new apps have it; `wisp test --browser`) a test
drives real headless Chrome or Edge over the DevTools protocol; no Node.

```rust
#[test]
fn counter() {
    let mut b = wisp::browser!(App); // the app on a free port, and a browser
    b.goto("/");
    b.click("text=Plus One");
    assert_eq!(b.text("output"), "1");
}
```

`goto(path) click(sel) hover(sel) fill(sel, text) press(key) text(sel)
attr(sel, name) count(sel) wait(sel) eval(js) -> Value url()
screenshot(path) timeout(d)`. Selectors: CSS, or `text=Plus One` (innermost
element whose text, `aria-label` or `title` contains it, any case). Actions
wait for the element (there, visible, enabled, uncovered) and for the page
to settle; a wait over 5 s fails with the address and DOM. Browser:
`$WISP_BROWSER`, else Chrome, Edge, Chromium or Brave; none: `browser!`
returns and the test passes, skipped. `wisp::test::browser::<App>()` is an
`Option<Browser>`. `--no-sandbox` on Linux.

## Where it runs

All of this works in the binary, Docker, Lambda and `tower`. The edge
(`--target cloudflare` etc.) runs each request in an instance that may be
its own: `wisp::channel`, `wisp::every`, `RateLimit` are not there,
WebSockets answer 501 (use the host's queues, cron, rate limiting), and
tables are per-instance memory. JSON, validation, errors, CORS, auth,
webhooks and docs work everywhere.

Dev logs every request, release logs failures. Pass a proxy's request id
back: `cx.set_header("x-request-id", id)` in `before` (`WISP_LOG=json`:
AGENTS.md).
