//! Projects that use every feature the compiler reads must pass `check`.

use crate::common::passes;

/// Every kind of route segment, layout and error page, at several depths.
#[test]
fn routes_of_every_shape() {
    passes(&[
        ("src/main.rs", "wisp::main!();"),
        (
            "src/params/word.rs",
            "fn matches(s: &str) -> bool { s.bytes().all(|b| b.is_ascii_lowercase()) }",
        ),
        (
            "src/routes/+layout.wisp",
            "<nav><a href=\"/\">home</a> {site}</nav>{@render children()}",
        ),
        (
            "src/routes/+layout.rs",
            "struct Data { site: String }\nfn load(cx: &Cx) -> Data { Data { site: cx.path().to_string() } }",
        ),
        (
            "src/routes/+error.wisp",
            "<h1>{status}</h1><p>{message}</p>",
        ),
        ("src/routes/+page.wisp", "<h1>home</h1>"),
        ("src/routes/about/+page.wisp", "<h1>about</h1>"),
        (
            "src/routes/blog/+error.wisp",
            "<h1>blog {status}</h1>{message}",
        ),
        ("src/routes/blog/+page.wisp", "blog"),
        ("src/routes/blog/[slug]/+page.wisp", "<h1>{slug}</h1>"),
        (
            "src/routes/blog/[slug]/comments/[id=int]/+page.wisp",
            "{slug} {id}",
        ),
        (
            "src/routes/shop/[[page=int]]/+page.wisp",
            "{#if let Some(p) = page}page {p}{/if}",
        ),
        (
            "src/routes/docs/[[lang]]/+page.wisp",
            "{format!(\"{lang:?}\")}",
        ),
        ("src/routes/files/[...rest]/+page.wisp", "{rest}"),
        ("src/routes/w/[w=word]/+page.wisp", "{w}"),
        (
            "src/routes/(marketing)/+layout.wisp",
            "<main>{@render children()}</main>",
        ),
        ("src/routes/(marketing)/pricing/+page.wisp", "pricing"),
        ("src/routes/(app)/(inner)/settings/+page.wisp", "settings"),
        (
            "src/routes/(app)/(inner)/settings/deep/+error.wisp",
            "deep {status}",
        ),
        (
            "src/routes/(app)/(inner)/settings/deep/+layout.wisp",
            "<div>{@render children()}</div>",
        ),
        ("src/routes/(app)/(inner)/settings/deep/x/+page.wisp", "x"),
        // Editor droppings are not routes.
        ("src/routes/.hidden/+page.wisp", "not a route"),
        ("src/routes/about/+page.wisp~", "backup"),
        ("src/routes/about/#lock", "lock"),
        ("src/routes/about/.+page.wisp.swp", "swap"),
    ]);
}

/// Route files with Rust: blocks, `+page.rs`, endpoints, hooks, modules.
#[test]
fn logic_of_every_shape() {
    passes(&[
        ("src/main.rs", "mod own;\nwisp::main!();"),
        ("src/own.rs", "pub fn f() {}"),
        (
            "src/shop.rs",
            "//! Prices.\n#![allow(dead_code)]\npub fn price(name: &str) -> u32 { if name == \"tea\" { 300 } else { 0 } }",
        ),
        (
            "src/models.rs",
            "#[derive(Json)]\npub struct Note { #[validate(len = 1..=20)] pub title: String, pub done: bool }",
        ),
        (
            "src/hooks.rs",
            "pub struct User(pub String);\n\
             async fn init() -> Result<()> { Ok(()) }\n\
             fn before(cx: &mut Cx) -> Result<Option<Response>> {\n    cx.set_header(\"x-app\", \"t\");\n    Ok(None)\n}\n\
             fn helper() {}",
        ),
        ("src/app.css", "body { margin: 0 }"),
        ("static/robots.txt", "User-agent: *"),
        ("static/img/a b.svg", "<svg/>"),
        // A block: actions with rules and route inputs, statements.
        (
            "src/routes/todos/+page.wisp",
            "---\nstatic TODOS: Table<String> = Table::new();\n\n\
             #[action]\nfn add(#[validate(len = 1..=10, email)] text: String, #[validate(min = 1, max = 9)] n: Option<u8>) {\n    TODOS.add(text);\n}\n\n\
             #[action]\nfn remove(id: u64) { TODOS.remove(id); }\n\n\
             #[action]\nfn export(title: String) -> Response { Response::text(title) }\n\n\
             #[action]\nfn maybe() -> Result<Option<Response>> { Ok(None) }\n\n\
             #[action]\nasync fn slow(cx: &mut Cx, tags: Vec<String>, on: bool, note: Option<&str>) -> Result {\n    Ok(())\n}\n\n\
             let count = TODOS.len();\n---\n\
             <title>Todos {count}</title>\n<form action=\"?/add\"><input name=\"text\">{cx.problem(\"text\")}</form>\n\
             {#each TODOS.all() as todo}<p>{todo}<button action=\"?/remove&id={todo.id}\">x</button></p>{/each}",
        ),
        // The long form: Rust beside the template, with its own docs and imports.
        (
            "src/routes/compat/+page.rs",
            "//! The long form.\n#![allow(dead_code)]\n\nuse wisp::prelude::*;\n\npub struct Data { pub who: String }\n\n\
             pub async fn load(cx: &mut Cx) -> Result<Data> {\n    match cx.query(\"who\") {\n        Some(w) => Ok(Data { who: w.to_string() }),\n        None => Err(Error::new(400, \"Who?\")),\n    }\n}",
        ),
        ("src/routes/compat/+page.wisp", "hi {who}"),
        // A limit, `entries`, and a route input.
        (
            "src/routes/up/[id=int]/+page.rs",
            "const BODY_LIMIT: usize = 4 * wisp::MB;\nstruct Data { id: u64, title: Option<String> }\n\
             fn load(id: u64, title: Option<String>) -> Data { Data { id, title } }\n\
             fn entries() -> Vec<Entry> { Vec::new() }\n\
             #[action]\nfn default(cx: &mut Cx) { cx.flash(\"ok\"); }",
        ),
        (
            "src/routes/up/[id=int]/+page.wisp",
            "<form method=\"post\" enctype=\"multipart/form-data\"><input name=\"title\" value={title}></form>{id}",
        ),
        // A route with an optional param and a `let` from the statements.
        (
            "src/routes/opt/[[n=int]]/+page.wisp",
            "---\nlet twice = n.map(|n| n * 2);\n---\n{format!(\"{twice:?}\")}",
        ),
        // Endpoints: handlers by method, a value, an id, `before`, a Rest type.
        (
            "src/routes/api/+server.rs",
            "const BODY_LIMIT: usize = 64 * wisp::KB;\n\
             /// Runs before each handler.\nfn before(cx: &mut Cx) -> Option<Response> { None }\n\
             fn get(n: Option<u8>) -> Vec<u8> { Vec::new() }\n\
             fn post(name: String) {}\n\
             fn put(body: Note) {}\n\
             fn patch(body: Option<String>) {}\n\
             fn delete() -> Option<Response> { None }",
        ),
        (
            "src/routes/tags/+server.rs",
            "static TAGS: Table<String> = Table::new();\n\
             fn list() -> Vec<Row<String>> { TAGS.all() }\n\
             fn post(name: String) -> u64 { TAGS.add(name) }\n\
             fn get(id: u64) -> Option<Row<String>> { TAGS.get(id) }\n\
             fn delete(id: u64) -> Option<()> { TAGS.remove(id).map(drop) }",
        ),
        (
            "src/routes/notes/+server.rs",
            "#[derive(Rest)]\n#[rest(write = \"TOKEN\")]\nstruct Note {\n    #[validate(len = 1..=20)]\n    title: String,\n    done: bool,\n}",
        ),
        (
            "src/routes/echo/+server.rs",
            "const BODY_LIMIT: usize = 64 * wisp::KB;\nfn post(cx: &mut Cx) -> Response { Response::text(\"\") }",
        ),
        (
            "src/routes/files/[...name]/+server.rs",
            "async fn get(name: String) -> Result<Response> { Response::file_in(\"static\", &name).await }",
        ),
        (
            "src/routes/sugar/+page.rs",
            "struct Data { href: &'static str, on: bool, n: u32 }\nfn load() -> Data { Data { href: \"/x\", on: true, n: 7 } }",
        ),
        (
            "src/routes/sugar/+page.wisp",
            "<a {href} class=\"x\" class:on={on} hidden={on}>{n}</a>",
        ),
        (
            "src/routes/login/+page.wisp",
            "---\n#[action]\nfn default(name: String) -> Result {\n    if name.is_empty() { return error(400, \"A name\"); }\n    redirect(\"/\")\n}\n---\n<form method=\"post\"><input name=\"name\"></form>",
        ),
        (
            "src/routes/blocks/+layout.wisp",
            "---\nconst A: u8 = 1;\nlet path = cx.path();\n---\n<p>{path}{A}</p>{@render children()}",
        ),
        (
            "src/routes/blocks/+page.wisp",
            "---\nfn twice(n: u32) -> u32 { n * 2 }\nlet a = twice(2);\n---\n{a}",
        ),
    ]);
}

/// Components, snippets, browser code and the JavaScript beside them.
#[test]
fn templates_of_every_shape() {
    passes(&[
        (
            "src/lib/names.js",
            "export const label = (s) => s.toUpperCase()\n",
        ),
        (
            "src/lib/nested/cart.js",
            "import { store, persisted, derived } from 'wisp'\nimport { label } from '../names.js'\n\
             export const cart = store([])\nexport const theme = persisted('t', 'light')\nexport const size = derived(() => cart.value.length)\nexport const title = label('cart')\n",
        ),
        (
            "src/components/Badge.wisp",
            "{@props label: impl std::fmt::Display}\n<span class=\"badge\">{label}</span>",
        ),
        (
            "src/components/Card.wisp",
            "{@props title: &str, count: u32 = 0, featured: bool = false}\n<section><h2>{title}{#if featured} *{/if}</h2><p>{count}</p>{@render children()}</section>",
        ),
        (
            "src/components/Box.wisp",
            "<div class=\"box\"><slot /><form action=\"?/add\"><input name=\"text\"></form></div>",
        ),
        (
            "src/components/Table.wisp",
            "{@props rows: &[(&str, u32)], row: Snippet<&(&str, u32), usize>}\n<table>{#each rows as r, i}<tr>{@render row(r, i)}</tr>{/each}{@render children()}</table>",
        ),
        (
            "src/components/Item.wisp",
            "{@props label: &str, count: u32 = 0}\n<li><span>{:label}</span><button on:click=\"count++; emit('bump', label)\">{:count}</button>{@render children()}<em>{:ctx}</em></li>\n<script>\n  const ctx = getContext('list')\n</script>",
        ),
        (
            "src/components/Tally.wisp",
            "<button on:click=\"n++\">{:n}</button>\n<script>\n  let n = 0\n</script>",
        ),
        (
            "src/components/Chart.wisp",
            "{@props label: &str}\n<div data-label={label}><button on:click=\"n++\">{label}: {:n}</button></div>\n<script>\n  let n = 0\n</script>",
        ),
        (
            "src/components/Stepper.wisp",
            "{@props value: i32 = 0, step: i32 = 1}\n<button on:click=\"value += step\">{:value}</button>\n<script>\n  let { value = $bindable(0), step = 1 } = $props()\n</script>",
        ),
        (
            "src/components/Pill.wisp",
            "<span class={:['pill', tone, { on }]} {:...rest}>{:text}</span>\n<script>\n  let { label: text, tone = 'plain', on = false, ...rest } = $props()\n</script>",
        ),
        (
            "src/components/Tree.wisp",
            "{@props node: &str = \"\"}\n<li>{:node.name}{:#if node.kids && node.kids.length}<ul>{:#each node.kids as kid (kid.name)}<Tree node={:kid} />{/each}</ul>{/if}</li>",
        ),
        (
            "src/components/Who.wisp",
            "<p :text=\"who\"></p>\n<script>\n  const who = getContext('who') ?? 'none'\n</script>",
        ),
        ("src/components/sub/Deep.wisp", "<i>deep</i>"),
        ("src/components/Card.wisp.bak", "editor backup"),
        ("src/components/notes.txt", "not a component"),
        (
            "src/routes/+page.wisp",
            "---\nlet items: Vec<(&str, u32)> = vec![(\"pen\", 2), (\"ink\", 5)];\nlet greeting = \"hi\";\n---\n\
             <title>Home</title>\n<wisp:head><meta name=\"x\" content=\"y\"></wisp:head>\n\
             <Card title={greeting} count={3} featured><Badge label=\"new\" /><Badge label={greeting.len()} /></Card>\n<Card title=\"Plain\" />\n\
             <Box>boxed</Box><Deep />\n\
             {#snippet cell(name, qty)}<td>{name}</td><td>{qty}</td>{/snippet}\n\
             {#snippet line(item: &(&str, u32), i: usize)}<td>{i}</td>{@render cell(item.0, item.1)}{/snippet}\n\
             <table>{#each items as it, i}<tr>{@render line(it, i)}</tr>{/each}</table>\n\
             <Table rows={items} row={line} />\n\
             <Table rows={items}>{#snippet row(r, i)}<td>{i}:{r.0}</td>{/snippet}<caption>kids</caption></Table>\n\
             {#snippet chip(label)}<b>{:label}</b>{/snippet}\n<p>{:#each tags as tag}{:@render chip(tag)}{:/each}</p>\n\
             {#match items.len()}{:case 0}none{:case _}some{/match}\n\
             {#each items as it}{it.0}{:else}empty{/each}\n\
             {@html \"<b>raw</b>\"}\n{@const n = items.len()}{n}\n\
             <script>\n  let tags = ['x', 'y']\n</script>",
        ),
        (
            "src/routes/runes/+page.wisp",
            "<p>{:count}</p><p>{:double}</p><p>{:done} of {:todos.length}</p>\n\
             <ul>{:#each todos as todo (todo.id)}<li class:done=\"todo.done\" on:click=\"todo.done = !todo.done\">{:todo.text}</li>{:/each}</ul>\n\
             <button on:click=\"count++\">+1</button><p>{:$cart.length}</p><Stepper bind:value=\"count\" step={:2} />\n\
             <script>\n  import { cart } from '$lib/nested/cart.js'\n  let count = $state(0)\n  let todos = $state([{ id: 1, text: 'one', done: false }])\n  let double = $derived(count * 2)\n  let done = $derived(todos.filter((t) => t.done).length)\n  $effect(() => { document.title = `Runes ${count}` })\n  $inspect(count)\n</script>",
        ),
        (
            "src/routes/islands/+page.wisp",
            "<Chart label=\"idle\" client:idle /><Chart label=\"wide\" client:media=\"(min-width: 1px)\" /><Stepper client:interaction /><Chart label=\"never\" client:none /><Tally client:visible />",
        ),
        (
            "src/routes/more/+page.wisp",
            "<wisp:window bind:innerWidth on:keydown.k=\"keys++\" />\n<p>{:innerWidth > 0}</p>\n\
             <Pill label=\"served\" tone=\"warm\" title=\"tip\" />\n\
             {:#key version}<p>v{:version}</p>{:/key}\n\
             {:#await slow}<p>wait</p>{:then v}<p>{:v}</p>{:catch e}<p>{:e.message}</p>{:/await}\n\
             {:#try}<p>{:risky()}</p>{:catch e}<p>{:e.message}</p>{:/try}\n\
             <input type=\"radio\" value=\"s\" bind:group=\"size\"><div contenteditable bind:innerHTML=\"html\"></div>\n\
             <div bind:clientWidth=\"w\">box</div><p style={:{ color: hue }} {:...attrs}>styled</p>\n\
             <wisp:element this={:tag}>dynamic</wisp:element>\n\
             <div use:portal=\"'#t'\">ported</div>{:#if shown}<p in:fade out:spin>fx</p>{:/if}\n\
             {:#each items as item, i (item.id)}<li animate:flip data-id={:item.id}>{:i}</li>{:else}<li>none</li>{:/each}\n\
             <script>\n  let keys = 0\n  let innerWidth\n  let version = 1\n  let slow = Promise.resolve(1)\n  const risky = () => 'fine'\n  let size = 's'\n  let html = ''\n  let w = 0\n  let hue = 'red'\n  let attrs = {}\n  let tag = 'h2'\n  let shown = false\n  let items = []\n  function spin(el) { return { duration: 50, css: (t) => `opacity: ${t}` } }\n</script>",
        ),
        (
            "src/routes/enhance/+page.wisp",
            "---\n#[action]\nasync fn add(text: String) {}\n#[action]\nfn answer() -> Response { Response::json(\"{}\") }\n---\n\
             <form method=\"post\" action=\"?/add\" use:enhance=\"submit\"><input name=\"text\" bind:value=\"text\"><button>Send</button></form>\n\
             <form method=\"post\" action=\"?/answer\" use:enhance=\"json\"><button>JSON</button></form><p>{:pending}</p>\n\
             <script>\n  let text = ''\n  let pending = false\n  function submit({ formData }) { pending = true; return (r) => { pending = false } }\n  function json() { return (r) => {} }\n</script>",
        ),
        // Browser code that reads the page's data, and a `+page.js` load.
        (
            "src/routes/load/+page.rs",
            "#[derive(Json)]\nstruct Data { server: String }\nfn load() -> Data { Data { server: \"s\".into() } }",
        ),
        (
            "src/routes/load/+page.js",
            "import { label } from '$lib/names.js'\nexport async function load({ data, url }) { return { from: data.server, shout: label('hi') } }\n",
        ),
        (
            "src/routes/load/+page.wisp",
            "<p>{:data.from}</p><p>{:data.shout}</p><Who />\n<script>\n  setContext('who', data.from)\n</script>",
        ),
        (
            "src/routes/paint/+page.rs",
            "#[derive(Json)]\nstruct Todo { id: u32, text: String, done: bool }\n#[derive(Json)]\nstruct Node { name: String, kids: Vec<Node> }\n\
             #[derive(Json)]\nstruct Data { title: String, todos: Vec<Todo>, empty: Vec<u32>, tree: Node }\n\
             fn load() -> Data { Data { title: String::new(), todos: Vec::new(), empty: Vec::new(), tree: Node { name: String::new(), kids: Vec::new() } } }",
        ),
        (
            "src/routes/paint/+page.wisp",
            "<h1>{:title}</h1>\n<ul>{:#each todos as todo, i (todo.id)}<li class=\"todo\" class:done=\"todo.done\">{:i}. {:todo.text}{:#if todo.done} <b>done</b>{/if}</li>{:else}<li>none</li>{/each}</ul>\n\
             <ul><Tree node={:data.tree} /></ul>\n<ul>{:#each todos as todo (todo.id)}<Item label={:todo.text} count={:todo.id}><i>{:todo.id}</i></Item>{/each}</ul>\n\
             {#each data.todos as t}<p>{:#if t.done}<span>{t.text}</span>{/if}</p>{/each}\n\
             <button on:click=\"todos = [...todos, { id: 1, text: 'new', done: false }]\">Add</button>\n\
             <script>\n  let todos = data.todos\n  let title = data.title\n  setContext('list', 'paint')\n</script>",
        ),
        (
            "src/routes/nav/+layout.wisp",
            "<nav><a href=\"/nav/one\">One</a><button on:click=\"n++\">{:n}</button><span>{:page.value.url.pathname}</span></nav>{@render children()}\n<script>\n  let n = 0\n  setContext('who', 'layout')\n</script>",
        ),
        ("src/routes/nav/one/+page.wisp", "one"),
    ]);
}

/// An app with nothing in it is still an app.
#[test]
fn an_empty_project_passes() {
    passes(&[]);
    passes(&[(
        "src/app.html",
        "<html><head>%wisp.head%</head><body>%wisp.body%</body></html>",
    )]);
    passes(&[("src/routes/+page.wisp", "")]);
}

/// Windows checkouts and editors leave `\r\n` and a byte order mark.
#[test]
fn crlf_and_bom_are_read_as_text() {
    passes(&[
        (
            "src/routes/+layout.wisp",
            "\u{feff}<main>\r\n{@render children()}\r\n</main>",
        ),
        (
            "src/routes/+page.wisp",
            "---\r\nlet a = 1;\r\n---\r\n<p>{a}</p>\r\n",
        ),
        ("src/routes/x/+page.wisp", "hi"),
        (
            "src/routes/x/+page.rs",
            "\u{feff}struct Data;\r\nfn load() -> Data { Data }\r\n",
        ),
        ("src/hooks.rs", "\u{feff}fn init() {}\r\n"),
    ]);
}

/// Forms in components, props of every kind, and the less usual branches.
#[test]
fn more_shapes() {
    passes(&[
        ("src/lib.rs", "wisp::app!();"),
        (".wisp/app.css", "a{}"),
        (
            "src/components/Form.wisp",
            "{@props open: bool = false, xs: &[u8]}\n\
             {#if open}<form action=\"?/a\"><input name=\"t\"></form>{:else}<form action=\"?/b\"><input name=\"t\"></form>{/if}\n\
             {#each xs as x}<form action=\"?/c\"><input name=\"t\"></form>{:else}<form action=\"?/d\"><input name=\"t\"></form>{/each}\n\
             {#match xs.len()}{:case 0}<form action=\"?/e\"><input name=\"t\"></form>{:case _}none{/match}\n\
             {#snippet s()}<form action=\"?/f\"><input name=\"t\"></form>{/snippet}{@render s()}\n\
             <Inner><form action=\"?/g\"><input name=\"t\"></form></Inner>",
        ),
        (
            "src/components/Inner.wisp",
            "<div>{@render children()}</div>",
        ),
        (
            "src/components/Count.wisp",
            "{@props n: u32 = 1, label: &str = \"x\"}\n<i>{label}{n}</i>",
        ),
        (
            "src/components/Pill.wisp",
            "<span {:...rest}>{:text}</span>\n<script>\n  let { label: text, on = false, ...rest } = $props()\n</script>",
        ),
        (
            "src/routes/+page.wisp",
            "---\nlet xs = vec![1u8, 2];\nlet t = \"text\";\nlet scheme = \"https\";\n\
             #[action]\nfn a(note: &'static str) {}\n---\n\
             <Form xs={&xs} open /><Form xs={&xs} />\n\
             <Count n=\"5\" label=\"y\" /><Count n={2} label={t} />\n\
             <Pill label=\"a\" hidden /><Pill label=\"b\" title={t} x=\"1\" />\n\
             {#if xs.is_empty()}empty{:else if xs.len() == 1}one{:else}many{/if}\n\
             <a href=\"{scheme}://x\">a</a><img src='java{t}'><form action=\"/x{#if t.is_empty()}{t}{/if}\"></form>\n\
             <a href={t} class=\"x {t}\" class:on={xs.is_empty()}>b</a>",
        ),
        (
            "src/routes/tags/+server.rs",
            "const BODY_LIMIT: usize = 1024;\nstatic TAGS: Table<String> = Table::new();\nfn list() -> Vec<Row<String>> { TAGS.all() }\nfn get(id: u64) -> Option<Row<String>> { TAGS.get(id) }",
        ),
        (
            "src/routes/notes/+server.rs",
            "const BODY_LIMIT: usize = 1024;\n#[derive(Rest)]\nstruct Note { title: String }",
        ),
    ]);
}

/// Browser code that reads what the page's statements bind, renders
/// components itself, and quotes what it writes.
#[test]
fn browser_shapes() {
    passes(&[
        (
            "src/components/Pill.wisp",
            "<span {:...rest}>{:text}</span>\n<script>\n  let { label: text, on = false, ...rest } = $props()\n</script>",
        ),
        (
            "src/components/Item.wisp",
            "{@props label: &str, count: u32 = 0}\n<li><button on:click=\"count++; emit('bump', label)\">{:count}</button></li>",
        ),
        (
            "src/components/Dyn.wisp",
            "{@props tag: &str}\n<wisp:element this={:tag}>x</wisp:element>",
        ),
        (
            "src/components/Slotty.wisp",
            "{@props row: Snippet, sep: char = ',', quoted: &str = \"a,b\", pair: (u8, u8) = (1, 2)}\n{@render row()}{sep}{quoted}",
        ),
        (
            "src/routes/state/+page.wisp",
            "---\nstatic COUNT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);\n\
             #[action]\nfn bump() {}\n\
             let count = COUNT.load(std::sync::atomic::Ordering::Relaxed);\nlet name = \"n\";\n---\n\
             <p>{count}</p><p>{:data.count}</p><p>{:data.name.length}</p>\n\
             <form method=\"post\" action=\"?/bump\"><button>Bump</button></form>\n\
             <script>\n  let bumps = 0\n  watch(() => data.count, () => bumps++)\n</script>",
        ),
        (
            "src/routes/each/+page.wisp",
            "{:#each xs as x, i (x.id + n)}\
             <Item label={:x.text} on:bump=\"last = event\" bind:count=\"counts[i]\" />\
             <Pill label=\"a\\b\nc\rd\u{7}\" hidden on={:x.done} />\
             <Dyn tag={:x.tag} />\
             <wisp:element this={:mk(x)}>e</wisp:element>\
             {:else}<p>none</p>{/each}\n\
             {:#each 3}<i>x</i>{/each}\n\
             {:#await p}<p>wait</p>{:then v}<p>{:v}</p>{:catch e}<p>{:e}</p>{/await}\n\
             {:#try}<p>{:p}</p>{:catch e}<p>{:e}</p>{/try}\n\
             <p title=\"a`b\\c$d {:n}\">t</p>\n\
             <Slotty>{#snippet row()}<b>r</b>{/snippet}</Slotty>\n\
             <script>\n  let xs = [{ id: 1, text: 't', tag: 'b', done: false }]\n  let n = 0\n  let counts = []\n  let last = ''\n  let p = Promise.resolve(1)\n  const mk = (x) => x.tag\n</script>",
        ),
    ]);
}
