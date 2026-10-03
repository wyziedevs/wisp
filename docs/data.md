# Data, files and jobs

Everything here is opt-in: an app that uses none of it pays nothing.

## Tables

```rust
// src/db.rs
pub static USERS: Table<User> = Table::saved("users")
    .unique("email", |u: &User| &u.email)   // no two rows share it
    .migrate(|row| rename(row, "mail", "email"))  // old rows, as they are read
    .live();                                 // each change is sent on channel "users"
```

- `set(id, v) -> Option<()>` replaces a row; `clear()` takes every row out
  (ids are not given again).
- `unique`: `by(&email)` finds the row without a scan; `try_add`,
  `try_set` and `try_update` return a 422 on the field (`email: is taken`),
  for an action to return with `?`. `add`, `set` and `update` panic (a 500)
  instead. Use `try_update` to change a unique field.
- `migrate(|v: &mut Value| ..)` runs on each stored row's JSON when it is
  read (at load, and by `WISP_STORE_POLL`), before it becomes the type.
- `live()`: after each change, `wisp::channel(name).send("change")`, only
  when someone listens. A page listens with `listen('/users/events',
  invalidate)` and a `+server.rs` of `wisp::channel("users").events()`.
- Several servers on one store: a `Store` may answer `changes(table,
  since)`, and with `WISP_STORE_POLL=5` (seconds) every table follows it.
  Tables are whole in memory: the store's size is the RAM bound.

## Relay

`wisp::relay(impl Relay)` in `init` carries every channel's messages to the
other servers (Redis `PUBLISH`, Postgres `NOTIFY`, NATS): two methods,
`publish(channel, text)` and `subscribe(deliver)`. Each message carries the
sender's id, so a broker that echoes back is fine.

## Files

`Upload` keeps any file as a blob, by the hash of its bytes (one copy of
equal files); a row holds its hash, name, type and size, and it shows as its
URL: `<a href={doc.file}>`.

```rust
let file = cx.form().file("file").or_status(400)?;
DOCS.add(Doc { title, file: Upload::new(&file, "pdf csv")? });  // 422 on `file`
```

Files go in `WISP_BLOBS` (default: `blobs` beside the data folder; memory
where tables are); `wisp::blobs(impl Blobs)` puts them in S3 or elsewhere.
`wisp::blob::serve(path)` answers `/_wisp/blob/<hash>`: typed by its bytes
(images) or as opaque data, `nosniff`, cached for good.

## Rules

`#[validate(url, one_of = "a b c", pattern = "[a-z]{3}-\\d+", with = ok)]`
beside `len min max min_len max_len email`. `url` is an absolute http(s)
address. `pattern` matches the whole value: literals, `.`, `[a-z_]`, `[^x]`,
`\d \w \s`, groups, `|`, `? * + {n} {n,m}` (values up to 1000 characters;
too much backtracking does not match). `with = f` calls `f(&v)
-> Option<String>`, the problem.

## Jobs

```rust
// init
wisp::work("mail", |m: Mail| async move { send(&m).await });
wisp::cron("0 3 * * *", || async { purge().await });  // UTC
// anywhere
wisp::queue("mail").push(&Mail { to, body });         // .later(secs, &job)
```

Jobs live in the saved table `queue-mail`. A failure (`Err` or a panic) is
tried again after 4, 8, 16... seconds (an hour at most), five tries; then it
stays `dead` with its last error (`queue.dead()`, `queue.retry(id)`).
At least once: a job running when the process died runs again a minute
later. One at a time per queue, in order. `cron` takes five fields (`*`,
`n`, `a-b`, `*/n`, lists; Sunday is 0 or 7).

## Cache

`wisp::cache("top", 60, || async { top_posts().await }).await` keeps an
answer for 60 s per process. `wisp::uncache("/posts")` forgets cache keys
starting with it and the pages `const CACHE` keeps, on every worker, for
`/posts` and below (`/` is all).

## Admin

With `WISP_ADMIN_KEY` set, `/_wisp/admin` (HTTP basic auth, the key as the
password) lists the saved tables, edits a row's JSON and deletes rows. It
writes rows directly: no hooks run, only that the JSON is a row of the type
(and unique fields are free). Form posts need this site's own `Origin`.
