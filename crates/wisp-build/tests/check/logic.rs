//! What the compiler refuses in an app's Rust, and in how routes are put
//! together: layouts, pages, endpoints, hooks, modules.

use crate::common::fails;

const HOME: (&str, &str) = ("src/routes/+page.wisp", "x");

#[test]
fn layouts_and_error_pages() {
    fails(&[
        (
            "layout without children",
            &[("src/routes/+layout.wisp", "<div></div>"), HOME],
            &["src/routes/+layout.wisp: a layout must contain {@render children()}"],
        ),
        (
            "layout with props",
            &[
                (
                    "src/routes/+layout.wisp",
                    "\n{@props a: u8}\n{@render children()}",
                ),
                HOME,
            ],
            &["src/routes/+layout.wisp:2: only components, in src/components, take props"],
        ),
        (
            "action in a layout block",
            &[
                (
                    "src/routes/+layout.wisp",
                    "---\n#[action]\nfn a() {}\n---\n{@render children()}",
                ),
                HOME,
            ],
            &["src/routes/+layout.wisp:3: layouts cannot have actions (`a`); put it in the page"],
        ),
        (
            "action in a +layout.rs",
            &[
                ("src/routes/+layout.wisp", "{@render children()}"),
                ("src/routes/+layout.rs", "\n#[action]\nfn b() {}"),
                HOME,
            ],
            &["src/routes/+layout.rs:3: layouts cannot have actions (`b`)"],
        ),
        (
            "BODY_LIMIT in a layout block",
            &[
                (
                    "src/routes/+layout.wisp",
                    "---\nconst BODY_LIMIT: usize = 1;\n---\n{@render children()}",
                ),
                HOME,
            ],
            &["src/routes/+layout.wisp:2: a layout's `BODY_LIMIT` does nothing"],
        ),
        (
            "a block and a +layout.rs",
            &[
                (
                    "src/routes/+layout.wisp",
                    "---\nlet a = 1;\n---\n{@render children()}",
                ),
                ("src/routes/+layout.rs", ""),
                HOME,
            ],
            &[
                "src/routes/+layout.wisp: this file starts with a `---` block of Rust, and +layout.rs is beside it",
            ],
        ),
        (
            "layout load returns nothing",
            &[
                ("src/routes/+layout.wisp", "{@render children()}"),
                ("src/routes/+layout.rs", "fn load() {}"),
                HOME,
            ],
            &["src/routes/+layout.rs:1: `load` returns nothing, but the template reads a `Data`"],
        ),
        (
            "layout load returns the wrong type",
            &[
                ("src/routes/+layout.wisp", "{@render children()}"),
                (
                    "src/routes/+layout.rs",
                    "\nfn load() -> String { String::new() }",
                ),
                HOME,
            ],
            &["src/routes/+layout.rs:2: `load` returns String, but the template reads a `Data`"],
        ),
        (
            "error page with a block",
            &[
                ("src/routes/+error.wisp", "---\nlet a = 1;\n---\n{status}"),
                HOME,
            ],
            &["src/routes/+error.wisp: an error page shows `status` and `message`"],
        ),
        (
            "error page with children",
            &[("src/routes/+error.wisp", "{@render children()}"), HOME],
            &["src/routes/+error.wisp: only layouts and components can use {@render children()}"],
        ),
        (
            "error page with props",
            &[("src/routes/+error.wisp", "{@props a: u8}"), HOME],
            &["src/routes/+error.wisp:1: only components, in src/components, take props"],
        ),
        (
            "error page with a broken template",
            &[("src/routes/+error.wisp", "\n<p>{#if x}</p>"), HOME],
            &["src/routes/+error.wisp:2:"],
        ),
        (
            "layout with a broken template",
            &[
                ("src/routes/+layout.wisp", "{@render children()}{/if}"),
                HOME,
            ],
            &["src/routes/+layout.wisp:1:", "no block to close"],
        ),
    ]);
}

#[test]
fn pages() {
    fails(&[
        (
            "page with children",
            &[("src/routes/+page.wisp", "{@render children()}")],
            &["src/routes/+page.wisp: only layouts and components can use {@render children()}"],
        ),
        (
            "page with props",
            &[("src/routes/+page.wisp", "\n\n{@props a: u8}")],
            &["src/routes/+page.wisp:3: only components, in src/components, take props"],
        ),
        (
            "a block never closed",
            &[("src/routes/+page.wisp", "\n---\nlet a = 1;\n")],
            &[
                "src/routes/+page.wisp:2:1: this `---` starts a block of Rust, which needs a `---` line after it",
            ],
        ),
        (
            "a block and a +page.rs",
            &[
                ("src/routes/+page.wisp", "---\nlet a = 1;\n---\n"),
                ("src/routes/+page.rs", ""),
            ],
            &[
                "src/routes/+page.wisp: this file starts with a `---` block",
                "+page.rs is beside it",
            ],
        ),
        (
            "statements and load in one block",
            &[(
                "src/routes/+page.wisp",
                "---\nstruct Data;\nfn load() -> Data { Data }\n\nlet a = 1;\n---\n",
            )],
            &[
                "src/routes/+page.wisp:5: the statements of a `---` block are the page's load",
                "(line 3)",
            ],
        ),
        (
            "a block with +page.js",
            &[
                ("src/routes/+page.wisp", "---\nlet a = 1;\n---\n{a}"),
                ("src/routes/+page.js", "export function load() {}"),
            ],
            &[
                "src/routes/+page.wisp: +page.js gets the page's `data`",
                "Move them into `fn load`",
            ],
        ),
        (
            "load returns nothing",
            &[
                ("src/routes/+page.wisp", "x"),
                ("src/routes/+page.rs", "fn load() {}"),
            ],
            &["src/routes/+page.rs:1: `load` returns nothing, but the template reads a `Data`"],
        ),
        (
            "load returns a block's wrong type",
            &[(
                "src/routes/+page.wisp",
                "---\n\nfn load() -> Vec<u8> { Vec::new() }\n---\nx",
            )],
            &["src/routes/+page.wisp:3: `load` returns Vec<u8>, but the template reads a `Data`"],
        ),
        (
            "load that is an action",
            &[
                ("src/routes/+page.wisp", "x"),
                (
                    "src/routes/+page.rs",
                    "struct Data;\n#[action]\nfn load() -> Data { Data }",
                ),
            ],
            &["src/routes/+page.rs:3: `load` cannot be both load and an action"],
        ),
        (
            "an entries with a parameter",
            &[
                ("src/routes/[a]/+page.wisp", "x"),
                (
                    "src/routes/[a]/+page.rs",
                    "\nfn entries(n: u8) -> Vec<u8> { Vec::new() }",
                ),
            ],
            &["src/routes/[a]/+page.rs:2: `entries` is `fn entries() -> Vec<...>`"],
        ),
        (
            "an async entries",
            &[
                ("src/routes/[a]/+page.wisp", "x"),
                (
                    "src/routes/[a]/+page.rs",
                    "async fn entries() -> Vec<u8> { Vec::new() }",
                ),
            ],
            &["`entries` is `fn entries() -> Vec<...>`: no parameters, not async, not an action"],
        ),
        (
            "an entries that is an action",
            &[
                ("src/routes/[a]/+page.wisp", "x"),
                ("src/routes/[a]/+page.rs", "#[action]\nfn entries() {}"),
            ],
            &["`entries` is `fn entries() -> Vec<...>`"],
        ),
        (
            "an action that returns a value",
            &[
                ("src/routes/+page.wisp", "x"),
                ("src/routes/+page.rs", "\n#[action]\nfn a() -> u8 { 1 }"),
            ],
            &[
                "src/routes/+page.rs:3: action `a` returns `u8`",
                "or a `Response` to send instead of the page",
            ],
        ),
        (
            "an action in a block that takes a pattern",
            &[(
                "src/routes/+page.wisp",
                "---\n#[action]\nfn a((x, y): (u8, u8)) {}\n---\nx",
            )],
            &[
                "src/routes/+page.wisp:3: `a` takes `(x, y): (u8, u8)`",
                "read from the request by its name",
            ],
        ),
        (
            "an action in a block inside a function",
            &[(
                "src/routes/+page.wisp",
                "---\nfn outer() {\n    #[action]\n    fn a() {}\n}\n---\nx",
            )],
            &["src/routes/+page.wisp:3: #[action] marks a top-level function of +page.rs"],
        ),
        (
            "Err(error(..)) in a +page.rs",
            &[
                ("src/routes/+page.wisp", "x"),
                (
                    "src/routes/+page.rs",
                    "\n#[action] fn a() -> Result<()> { Err(error(400, \"no\")) }",
                ),
            ],
            &[
                "src/routes/+page.rs:2: `error()` returns the `Result` itself",
                "Error::new(status, message)",
            ],
        ),
        (
            "Err(redirect(..)) in a block",
            &[(
                "src/routes/+page.wisp",
                "---\n#[action]\nfn a() -> Result<()> { Err(redirect(\"/\")) }\n---\nx",
            )],
            &[
                "src/routes/+page.wisp:3: `redirect()` returns the `Result` itself",
                "Error::redirect(status, location)",
            ],
        ),
        (
            "inner attribute after an item",
            &[
                ("src/routes/+page.wisp", "x"),
                ("src/routes/+page.rs", "fn a() {}\n#![allow(dead_code)]"),
            ],
            &["src/routes/+page.rs:2: `//!` docs and `#![…]` attributes go at the top of the file"],
        ),
        (
            "BODY_LIMIT of the wrong type",
            &[
                ("src/routes/+page.wisp", "x"),
                ("src/routes/+page.rs", "\npub const BODY_LIMIT: u64 = 1;"),
            ],
            &["src/routes/+page.rs:2: `BODY_LIMIT` is a `u64`; make it a `usize`"],
        ),
        (
            "BODY_LIMIT in a page and its server",
            &[
                ("src/routes/x/+page.wisp", "x"),
                ("src/routes/x/+page.rs", "const BODY_LIMIT: usize = 1;"),
                (
                    "src/routes/x/+server.rs",
                    "\nconst BODY_LIMIT: usize = 2;\nfn put() {}",
                ),
            ],
            &["src/routes/x/+server.rs:2: `BODY_LIMIT` is also set in this route's +page.rs"],
        ),
    ]);
}

#[test]
fn action_inputs_are_checked() {
    let page = |rs: &'static str| -> [(&'static str, &'static str); 2] {
        [("src/routes/+page.wisp", "x"), ("src/routes/+page.rs", rs)]
    };
    fails(&[
        (
            "unknown rule",
            &page("\n#[action]\nfn a(#[validate(size = 1)] t: String) {}"),
            &[
                "src/routes/+page.rs:3: #[validate] has no `size`",
                "len, min, max, min_len, max_len, email, url, one_of, pattern, with and max_size",
            ],
        ),
        (
            "len without a range",
            &page("#[action]\nfn a(#[validate(len = 5)] t: String) {}"),
            &["src/routes/+page.rs:2: `len = 5` needs a range, such as `len = 1..=100`"],
        ),
        (
            "len without bounds",
            &page("#[action]\nfn a(#[validate(len = ..)] t: String) {}"),
            &["`len = ..` needs a bound"],
        ),
        (
            "rules that need a value",
            &page("#[action]\nfn a(#[validate(min)] n: u8) {}"),
            &["`min` needs a value: `min = 1`"],
        ),
        (
            "len that needs a value",
            &page("#[action]\nfn a(#[validate(len)] t: String) {}"),
            &["`len` needs a value"],
        ),
        (
            "email with a value",
            &page("#[action]\nfn a(#[validate(email = 1)] t: String) {}"),
            &["`email` takes no value"],
        ),
        (
            "rules on what is not read from the request",
            &page("#[action]\nfn a(#[validate(min = 1)] cx: &mut Cx) {}"),
            &[
                "src/routes/+page.rs:2: `#[validate]` is on `cx`, which is not read from the request",
            ],
        ),
        (
            "rules on a load",
            &page("struct Data;\nfn load(#[validate(min = 1)] n: u8) -> Data { Data }"),
            &["src/routes/+page.rs:2: `#[validate]` on a parameter checks an action's input"],
        ),
        (
            "rules on an endpoint",
            &[(
                "src/routes/e/+server.rs",
                "\nfn post(#[validate(len = 1..)] n: String) {}",
            )],
            &["src/routes/e/+server.rs:2: `#[validate]` on a parameter checks an action's input"],
        ),
    ]);
    // Every rule, and its range forms, are accepted.
    crate::common::passes(&page(
        "#[action]\nfn a(\n    #[validate(len = ..10)] a: String,\n    #[validate(len = 2..)] b: String,\n    #[validate(len = 2..=3, min_len = 1, max_len = 9)] c: String,\n    #[validate(min = 1.5, max = 9)] d: f64,\n    #[validate(email)] e: String,\n    #[validate(len = 1..=3)] mut f: String,\n) {}",
    ));
}

#[test]
fn endpoints() {
    fails(&[
        (
            "an action",
            &[("src/routes/e/+server.rs", "\n#[action]\nfn post() {}")],
            &[
                "src/routes/e/+server.rs:3: `post` is marked #[action], but actions belong in a +page.rs",
            ],
        ),
        (
            "head",
            &[("src/routes/e/+server.rs", "fn get() {}\nfn head() {}")],
            &["src/routes/e/+server.rs:2: `head` is never called: HEAD is answered by `get`"],
        ),
        (
            "options",
            &[("src/routes/e/+server.rs", "fn get() {}\nfn options() {}")],
            &[
                "src/routes/e/+server.rs:2: `options` is never called",
                "OPTIONS by Wisp",
            ],
        ),
        (
            "before that returns a value",
            &[(
                "src/routes/e/+server.rs",
                "fn get() {}\nfn before(cx: &mut Cx) -> u8 { 1 }",
            )],
            &["src/routes/e/+server.rs:2: `before` returns `u8`"],
        ),
        (
            "before with an input",
            &[(
                "src/routes/e/+server.rs",
                "fn get() {}\nfn before(cx: &mut Cx, n: u8) {}",
            )],
            &["src/routes/e/+server.rs:2: `before` takes only `cx`"],
        ),
        (
            "list for an id",
            &[("src/routes/e/+server.rs", "fn list(id: u64) {}")],
            &[
                "src/routes/e/+server.rs:1: `list` answers GET on the route; `get(id: u64)` is the one for its `/[id]`",
            ],
        ),
        (
            "two handlers for a method",
            &[("src/routes/e/+server.rs", "fn get() {}\nfn list() {}")],
            &["src/routes/e/+server.rs:2: `list` and `get` both answer GET on the route"],
        ),
        (
            "two handlers for a method on an id",
            &[(
                "src/routes/e/+server.rs",
                "fn get(id: u64) {}\nfn delete(id: u64) {}\nfn get(id: i64) {}",
            )],
            &["both answer GET on `/[id]`"],
        ),
        (
            "no handlers",
            &[("src/routes/e/+server.rs", "fn helper() {}")],
            &["src/routes/e/+server.rs: defines none of get, post, put, patch, delete, list"],
        ),
        (
            "a Rest type under an id",
            &[(
                "src/routes/n/[id]/+server.rs",
                "#[derive(Rest)]\nstruct A { a: u8 }",
            )],
            &[
                "src/routes/n/[id]/+server.rs:",
                "`A` is served at this route and its `/[id]`, but the route already has an `id`",
            ],
        ),
        (
            "an endpoint that takes a pattern",
            &[("src/routes/e/+server.rs", "\nfn post((a, b): (u8, u8)) {}")],
            &["src/routes/e/+server.rs:2: `post` takes `(a, b): (u8, u8)`"],
        ),
        (
            "a server that does not scan",
            &[(
                "src/routes/e/+server.rs",
                "#[action]\nmod m { #[action] fn a() {} }",
            )],
            &["src/routes/e/+server.rs:2: #[action] marks a top-level function"],
        ),
        (
            "a get beside a page",
            &[
                ("src/routes/x/+page.wisp", "x"),
                ("src/routes/x/+server.rs", "fn get() {}"),
            ],
            &["src/routes/x/+server.rs: `get` conflicts with the page in the same directory"],
        ),
        (
            "a post beside a page with actions",
            &[
                (
                    "src/routes/x/+page.wisp",
                    "---\n#[action]\nfn a() {}\n---\nx",
                ),
                ("src/routes/x/+server.rs", "fn post() {}"),
            ],
            &["src/routes/x/+server.rs: `post` conflicts with the page in the same directory"],
        ),
        (
            "a list beside a page",
            &[
                ("src/routes/x/+page.wisp", "x"),
                ("src/routes/x/+server.rs", "fn list() {}"),
            ],
            &["`list` conflicts with the page"],
        ),
    ]);
    // A post beside a page without actions, and a get for an id, are fine.
    crate::common::passes(&[
        ("src/routes/x/+page.wisp", "x"),
        (
            "src/routes/x/+server.rs",
            "fn post() {}\nfn get(id: u64) {}",
        ),
    ]);
}

#[test]
fn hooks() {
    let hooks = |src: &'static str| [HOME, ("src/hooks.rs", src)];
    fails(&[
        (
            "an action",
            &hooks("\n#[action]\nfn init() {}"),
            &["src/hooks.rs:3: `init` is marked #[action], but actions belong in a +page.rs"],
        ),
        (
            "init with cx",
            &hooks("fn init(cx: &mut Cx) {}"),
            &[
                "src/hooks.rs:1: `init` runs once, before the server takes requests, so it has no `cx`",
            ],
        ),
        (
            "init that returns a value",
            &hooks("\nfn init() -> u8 { 1 }"),
            &["src/hooks.rs:2: `init` returns `u8`; it returns nothing, or `Result<()>`"],
        ),
        (
            "before that returns a value",
            &hooks("pub fn before(cx: &mut Cx) -> u8 { 1 }"),
            &["src/hooks.rs:1: `before` returns `u8`"],
        ),
        (
            "before with an input",
            &hooks("fn before(cx: &mut Cx, id: u8) {}"),
            &["src/hooks.rs:1: `before` takes only `cx`"],
        ),
        (
            "a public function that is not a hook",
            &hooks("\npub fn befor(cx: &mut Cx) {}"),
            &[
                "src/hooks.rs:2: `befor` is not a hook: src/hooks.rs has `init`, `before`, `after`, `report` and `reroute`",
            ],
        ),
        (
            "a late inner attribute",
            &hooks("fn init() {}\n#![allow(dead_code)]"),
            &["src/hooks.rs:2: `//!` docs and `#![…]` attributes go at the top"],
        ),
        (
            "a hook that does not scan",
            &hooks("fn a() {\n#[action]\nfn b() {}\n}"),
            &["src/hooks.rs:2: #[action] marks a top-level function"],
        ),
        (
            "an old error call",
            &hooks("fn init() -> Result<()> { Err(error(500, \"x\")) }"),
            &["src/hooks.rs:1: `error()` returns the `Result` itself"],
        ),
        (
            "mod hooks in main.rs",
            &[
                HOME,
                ("src/hooks.rs", "fn init() {}"),
                ("src/main.rs", "\nmod hooks;\nwisp::main!();"),
            ],
            &["src/main.rs:2: remove `mod hooks`: Wisp includes src/hooks.rs itself"],
        ),
        (
            "pub mod hooks in main.rs",
            &[HOME, ("src/main.rs", "pub mod hooks;")],
            &["src/main.rs:1: remove `mod hooks`"],
        ),
    ]);
    // Private helpers, both hooks, and a `mod hooks` in a comment are fine.
    crate::common::passes(&[
        HOME,
        ("src/main.rs", "// mod hooks;\nwisp::main!();"),
        (
            "src/hooks.rs",
            "fn helper() {}\nasync fn init() {}\nfn before(cx: &mut Cx) -> Response { Response::empty(204) }",
        ),
    ]);
}

#[test]
fn app_modules() {
    fails(&[
        (
            "an old error call",
            &[
                HOME,
                (
                    "src/notes.rs",
                    "\npub fn f() -> Result<()> { Err(error(400, \"x\")) }",
                ),
            ],
            &["src/notes.rs:2: `error()` returns the `Result` itself"],
        ),
        (
            "a late inner attribute",
            &[HOME, ("src/notes.rs", "pub fn f() {}\n//! Late docs.")],
            &["src/notes.rs:2: `//!` docs and `#![…]` attributes go at the top"],
        ),
        (
            "an action outside a top-level function",
            &[HOME, ("src/notes.rs", "mod m {\n#[action]\nfn a() {}\n}")],
            &["src/notes.rs:2: #[action] marks a top-level function"],
        ),
    ]);
    // Modules a `main.rs` or `lib.rs` declares are the app's own business;
    // so are files that are not modules (`Notes.rs`, `a-b.rs`, directories).
    crate::common::passes(&[
        HOME,
        ("src/main.rs", "mod declared;\nwisp::main!();"),
        ("src/lib.rs", "pub mod also;\nwisp::app!();"),
        (
            "src/declared.rs",
            "#![allow(dead_code)]\nfn a() { #[action] fn b() {} }\n//! nope",
        ),
        ("src/also.rs", "//! not scanned"),
        ("src/Notes.rs", "//! not a module"),
        ("src/a-b.rs", "//! not a module"),
        ("src/dir.rs/x.txt", "a directory named like a file"),
        ("src/readme.txt", "text"),
    ]);
}

#[test]
fn remote_functions() {
    let page = "---\n#[remote]\nfn twice(n: u32) -> u32 { n * 2 }\n---\n<p>x</p>";
    crate::common::passes(&[
        ("src/routes/+page.wisp", page),
        (
            "src/remote.rs",
            "#[remote(get)]\nfn find(id: u64) -> Option<String> { None }",
        ),
        (
            "src/lib/x.js",
            "import { find } from 'wisp:remote'\nexport const f = find",
        ),
    ]);
    fails(&[
        (
            "a name twice",
            &[
                ("src/routes/+page.wisp", page),
                (
                    "src/remote.rs",
                    "\n#[remote]\nfn twice(n: u32) -> u32 { n }",
                ),
            ],
            &["src/remote.rs:3: there is already a #[remote] fn `twice`"],
        ),
        (
            "a name JavaScript has",
            &[HOME, ("src/remote.rs", "#[remote]\nfn fetch() {}")],
            &["src/remote.rs:2: browser code calls #[remote] fn `fetch` by its name"],
        ),
        (
            "in a layout",
            &[
                HOME,
                (
                    "src/routes/+layout.wisp",
                    "---\n#[remote]\nfn a() {}\n---\n<slot />",
                ),
            ],
            &["src/routes/+layout.wisp:3: a layout cannot have #[remote] functions"],
        ),
        (
            "in +server.rs",
            &[("src/routes/e/+server.rs", "#[remote]\nfn get() {}")],
            &["src/routes/e/+server.rs:2: `get` is #[remote]"],
        ),
        (
            "in hooks.rs",
            &[HOME, ("src/hooks.rs", "#[remote]\nfn x() {}")],
            &["src/hooks.rs:2: `x` is marked #[remote]"],
        ),
        (
            "also an action",
            &[(
                "src/routes/+page.wisp",
                "---\n#[remote]\n#[action]\nfn a() {}\n---\nx",
            )],
            &["src/routes/+page.wisp:4: `a` cannot be both #[remote] and an action"],
        ),
        (
            "an import with none",
            &[HOME, ("src/lib/x.js", "import { a } from 'wisp:remote'")],
            &["src/lib/x.js: `wisp:remote` has the app's #[remote] functions"],
        ),
    ]);
}
