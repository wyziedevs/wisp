# Data and jobs

Everything here is opt-in: an app that uses none of it pays nothing. File storage, an admin UI and a cross-server relay are the app's: use a crate or a service.

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

On Cloudflare, Vercel and Netlify (`wisp build --target ...`) the same code
runs from the host's cron: `wisp build` writes each `cron` schedule (a string
literal) into its trigger config, and a trigger also runs the queues' due jobs
(each minute if the app has `work`). Set `CRON_SECRET` and `WISP_STORE` there.
See deploy.md.

## Cache

`wisp::cache("top", 60, || async { top_posts().await }).await` keeps an
answer for 60 s per process. `wisp::uncache("/posts")` forgets cache keys
starting with it and the pages `const CACHE` keeps, on every worker, for
`/posts` and below (`/` is all).
