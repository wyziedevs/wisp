<p align="center">
  <img src="examples/demo/static/favicon.svg" width="88" height="88" alt="">
</p>

<h1 align="center">Wisp</h1>

<p align="center">A fast, fun web framework for Rust.</p>

---

Wisp builds web apps from files: a folder is a URL, a `.wisp` template is its markup, and a `+page.rs` beside it holds the Rust that loads data and handles forms. The whole app, styles and static files included, compiles into one small binary.

- **Fast to build.** Markup edits appear in the browser in under 100 ms, without a recompile.
- **Fast to run.** Templates compile to plain Rust, with no virtual DOM and no runtime template engine.
- **Works without JavaScript.** Forms are real forms. A 4 KB script makes them update the page in place.
- **Type checked.** Every expression in a template is checked by the Rust compiler.
- **One file to deploy.** Copy the binary to a server and run it.

## Quick start

You need [Rust](https://rustup.rs) 1.88 or later.

```sh
cargo install --git https://github.com/wyziedevs/wisp wisp-cli
wisp new my-app
cd my-app
wisp dev
```

`wisp new` asks a few questions: a demo app or a blank one, Tailwind CSS, git, and whether to compile dependencies now. Then open http://127.0.0.1:3000.

## A page

```rust
// src/routes/+page.rs
use wisp::prelude::*;

pub struct Data {
    pub count: i64,
}

pub async fn load(cx: &mut Cx) -> Result<Data> {
    Ok(Data { count: count(cx) })
}

#[action]
pub async fn increment(cx: &mut Cx) -> Result<()> {
    let next = count(cx) + 1;
    cx.set_cookie("count", &next.to_string());
    Ok(())
}

fn count(cx: &Cx) -> i64 {
    cx.cookie("count").and_then(|c| c.parse().ok()).unwrap_or(0)
}
```

```html
<!-- src/routes/+page.wisp -->
<h1>Clicked {data.count} times</h1>

<form method="post" action="?/increment">
  <button>Click me</button>
</form>
```

## Routes

| File           | Purpose                                     |
| -------------- | ------------------------------------------- |
| `+page.wisp`   | The page's markup                           |
| `+page.rs`     | `load` for its data, `#[action]` for forms  |
| `+layout.wisp` | Wraps this page and every page below it     |
| `+error.wisp`  | Shown when something below it fails         |
| `+server.rs`   | Plain HTTP endpoints: `get`, `post`, ...    |

Folders named `[slug]` are parameters, `[...rest]` match the rest of the path, and `(group)` folders organize routes without changing the URL.

## Commands

| Command      | What it does                                         |
| ------------ | ---------------------------------------------------- |
| `wisp new`   | Create an app                                        |
| `wisp dev`   | Run it with hot reload                               |
| `wisp build` | Build one release binary with everything inside      |
| `wisp check` | Check routes and templates without compiling         |

## Performance

On a server-rendered HTML benchmark, Wisp serves 2.2× the requests of ASP.NET Core Razor Pages, using 6 MB of memory instead of about 100 MB. See [bench](bench/README.md) for the method and full results.

## Documentation

[docs/design.md](docs/design.md) covers the template language, routing, actions, the runtime and the dev server.

## License

[MIT](LICENSE)
