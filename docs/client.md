# Client code

Pages work without JavaScript; a script and directives in the same `.wisp`
file add behavior. No bundler. `{…}` is Rust (server); a quoted directive
value and `{:…}` are JavaScript (browser).

```html
<button on:click="count++">Clicked {:count} times</button>
<script>
  let count = 0
</script>
```

## The script
A bare `<script>` (no attributes; one per file) works in pages, layouts and
components and runs once per place the file is shown. `<script type|src>`
stays plain HTML. Errors point at the `.wisp` line.

- Top-level `let`s are state: assigning one, or changing an object, array,
  `Map` or `Set` in it (`todos.push(t)`, `todo.done = true`), redraws. A
  `let` set to a string, number or boolean and never assigned is a constant.
- `import` lines move to the module head (`import c from 'https://esm.sh/canvas-confetti'`).
- npm: `wisp add canvas-confetti[@1.2.3|tag]` pins it in `package.json`
  (`wisp remove x`; no Node); `import c from 'canvas-confetti'` (also
  `'pkg/sub'`, `'@scope/pkg'`) in scripts and `src/lib`. Dev loads esm.sh;
  `wisp build` downloads into `.wisp/npm` and the binary serves
  `/_app/c/npm/`. A package not in `package.json`, or a range (`^1.0`), is a
  build error.

### Runes

A write redraws only the bindings that read what changed (no virtual DOM);
the script runs once; writes batch in a microtask.

```html
<p>{:done} of {:todos.length} done</p>
{:#each todos as todo (todo.id)}
  <li class:done="todo.done" on:click="todo.done = !todo.done">{:todo.text}</li>
{:/each}
<script>
  let todos = $state([{ id: 1, text: 'Tea', done: false }])
  let done = $derived(todos.filter((t) => t.done).length)
  $effect(() => { document.title = `${done} done` })
</script>
```

| Rune | Meaning |
|---|---|
| `let x = $state(v)` | Deep state (a plain `let x = v` is the same). |
| `$state.raw(v)` | Changes only when assigned. |
| `$state.snapshot(x)` | Plain copy. |
| `$derived(expr)`, `$derived.by(fn)` | Recomputed when read after an input changed; assigning is a build error. |
| `$effect(fn)` | After the DOM is drawn and when what it read changes; may return a cleanup. |
| `$effect.pre(fn)` | Same, before the DOM is drawn. |
| `let { a, b = 1, c: d, ...rest } = $props()` | Component props with browser defaults (absent or `null`); needs no `{@props}`. |
| `$bindable(default)` | A prop a parent may `bind:`; with `$props()` only these bind. |
| `$inspect(a, b)` | Logs on change; gone in release. |
| `$cart` | Store `cart`'s `.value`, tracked; `$cart = x` sets it. |

A class's `$state`/`$derived` fields make its instances state. `untrack(fn)`
reads untracked. A misplaced rune is a build error. Deep state tracks plain
objects, arrays, maps, sets and such classes; for a `Date` or other
instance assign again (`d = d`). `x === e` redraws only where the answer
changes (`class:on="selected === row.id"` redraws two rows).

## TypeScript
`<script lang="ts">`, `src/lib/*.ts` (`'$lib/x'`) and `+page.ts`. Types are
stripped in place (lines and columns stay); no compiler. Code-producing TS
is a build error saying what to write: `enum Color { Red }` ->
`const Color = { Red: 'red' } as const`; `namespace` -> a module;
`constructor(private x: number)` -> `x: number; constructor(x: number) { this.x = x }`;
`import fs = require('fs')` -> `import fs from 'fs'`.
`wisp check --types` also type-checks with the app's TypeScript
(`npm install -D typescript`, or `WISP_TSC` naming a `tsc`; else skipped).
Server values are typed by Rust (`Vec<Item>` is `Item[]`, a
`#[derive(Json)]` type an interface; hand-written `Json` is `unknown`).

## Environment variables
`env.PUBLIC_NAME` in browser code (script, directive, `src/lib`, `+page.js`)
is written in at build (no `env` object exists). Values: the build's
environment, then `.env` for names it lacks; `wisp dev` rebuilds when `.env`
changes. Only `PUBLIC_` names reach the browser (`env.DATABASE_URL` is a
build error); an unset one is a build error too, so set it even empty
(`PUBLIC_FLAG=`). `env` read whole or `env[name]` is an error; a variable of
your own named `env` is just that. Server: `wisp::env("K")`.

## Translations
`t('cart.items', n)` or `t('hi', { name, count: n })` in a script or
directive, no import; keys checked at build; the page sends only the
messages its scripts use. `src/lib` code can't call `t`. Message files:
docs/design.md.

## Directives

| Syntax | Meaning |
|---|---|
| `on:click="count++"` | Handler; a bare name (`on:click="press"`) is called with the event. |
| `bind:value="q"` / `bind:checked="done"` | Two-way. `bind:value` alone binds `value`. An undeclared name is declared as state (`let q`): live search needs no `<script>`. |
| `bind:group="size"` | Radios (value) and checkboxes (array) sharing a `name`. |
| `bind:files` `bind:open` `bind:innerHTML` `bind:currentTime` `bind:paused`… | Any property; the element's own event keeps it current. |
| `bind:clientWidth="w"` | Sizes (`clientWidth/Height`, `offsetWidth/Height`, `contentRect`). |
| `bind:this="el"` | Element into `el`. |
| `:hidden="!open"` | Live attribute; `false`, `null`, `undefined` remove it. |
| `:text="name"` | Live text. |
| `class:open="isOpen"` | Toggle a class; `class:open` alone reads `open`. |
| `style:--x="x"` | Style property; `style:color` alone reads `color`. |
| `class={:['card', { on }]}` | Names from strings, arrays, truthy object keys. |
| `style={:{ color, fontSize: '2em' }}` | Properties from an object. |
| `{:...attrs}` | Each key an attribute (an `on…` function a listener). |
| `transition:fade` | `fade slide scale fly blur`, options `transition:fly="{ y: 20 }"`. |
| `in:fly` / `out:fade` | Only in / only out. |
| `transition:spin` | Your `spin(el, options, { direction })` returning `{ duration, delay, easing, css: (t, u) => '…' }` or `{ tick(t, u) }`. |
| `use:tip="'Hello'"` | Calls `tip(el, 'Hello')` and its `update` on change; may return a cleanup or `{ update, destroy }`. |
| `use:portal="'#modal'"` | Move the element there (bare: `<body>`). |
| `animate:flip` | Animate moves in a keyed `{:#each}`. |

```html
<input bind:value="query" on:keydown.enter="search" on:keydown.escape="query = ''">
<div on:click.outside="open = false">…</div>
<input on:input.debounce.300ms="search()">
<div on:keydown.ctrl.s.prevent.window="save">…</div>
```

### Event modifiers

`.prevent .stop .once .self .capture .passive`, `.window`
and `.document` (listen there), `.outside`, `.debounce[.300ms]` (250 ms),
keys (`.enter .escape .space .tab .backspace .delete .up .down .left
.right .home .end .pageup .pagedown`, a letter or digit), `.ctrl .shift
.alt .meta`. Unknown ones are a build error. A bound input starts from what
the server rendered or the visitor already typed.

## Server values in the browser
Any Rust value client code mentions is sent, only the mentioned parts.

```html
<script>
  let guess = data.guess          // the script's `guess`, from the server's
  const total = items.length      // the page's `let items` (or `data.items`)
</script>
{#each keys as key}<button on:click="type(key.letter)">{key.letter}</button>{/each}   <!-- sends key.letter only -->
```

- Sent: a page's or layout's Rust names (block `let`s, route params, `Data`
  fields) by name or `data.x.y`; component props; loop, `if let` and
  `{@const}` values used in a directive. A name the script declares is the
  script's; a browser global (`document`, `location`, `event`, `fetch`)
  stays the browser's (the Rust one is `data.location`); a parameter or
  local of that name (`items.map(data => data.x)`) is just that.
- Sent as JSON via `wisp::Json`: numbers, strings, `bool`, `Option`, `Vec`,
  arrays, tuples, maps; own types `#[derive(Json)]`. An unsendable value is
  a compile error naming `wisp::Json`; a prop that is also a script
  variable is a build error.
- `matches(text, q)` (no import): case-insensitive contains; empty `q`
  matches all. Live search: `<input bind:value="q">` then
  `{:#each items.filter((i) => matches(i.name, q)) as item}<p>{:item.name}</p>{:/each}`.

## `{:expr}` holes

A live JS expression anywhere in markup: `<p>Hi {:name}</p>`,
`<p class="card {:mood}" data-id={:item.id}>`, `<a href={:url}>`. Mixing
`{…}` and `{:…}` in one attribute value is a build error.

## Client blocks

```html
{:#if open}<p>Open</p>{:else if name}<p>{:name}</p>{:else}<p>Closed</p>{:/if}

{:#each items as item, i (item.id)}
  <li animate:flip>{:i}: {:item.text}</li>
{:else}
  <li>Nothing</li>
{:/each}

{:#key user.id}<Profile id={:user.id} />{:/key}      <!-- redrawn when it changes -->

{:#await results}<p>Loading…</p>{:then list}{:list.length}{:catch error}{:error.message}{:/await}
{:#await p then v}…{:/await}                        <!-- no pending branch -->

{:#try}<Chart data={:points} />{:catch error}
  <p>{:error.message}</p><button on:click="reset()">Retry</button>
{:/try}
```

`(item.id)` is the key (else by position). `{:#each list}` alone draws once
per item; `{:#each 3 as i}` counts. Also `<template each="item, i in list">`
and `<template if="cond">`. The server paints key blocks, an await's pending
branch and a try body.

Special elements (each takes directives for its target, closes itself):

```html
<wisp:window on:keydown.escape="open = false" bind:innerWidth="w" />
<wisp:document on:visibilitychange="save()" bind:visibilityState="seen" />
<wisp:body on:click="menu = false" />
<wisp:element this={:level > 1 ? 'h3' : 'h2'} class="title">{:text}</wisp:element>
```

Snippets: `{:@render name(args)}` draws a file's `{#snippet}` in the browser
(args are JS; parameters are plain names read in `{:…}`); `{@render}` still
works on the server.

```html
{#snippet chip(tag)}<b class="chip">{:tag}</b>{/snippet}
{:#each tags as tag (tag)}{:@render chip(tag)}{:/each}
```

### First paint

The server renders what it can know into the page (it works before JS and
without it); the browser takes those nodes over and keeps them live. Known:
server values, props, Rust loop values; literals (`0 'text' true null [1,2]
{id:1}`); script variables first set to those (`let todos = data.todos`); an
`{:#each}` item/index inside it; `!`, `&&`, `||`, `.length` of those; a
boolean directive like `:hidden="!open"` with `let open = false`. A call,
sum, comparison or a `+page.js` page's `data` is left to the browser.
Live attributes (`:class`, `class="a {:b}"`) keep static text until it starts.

## Client components
A component inside a client block, or given `{:…}`, `bind:` or `on:`, is
drawn by the browser.

```html
{:#each names as name (name)}
  <Item label={:name} bind:count="counts[name]" on:bump="bumped = event"><b>{:name}!</b></Item>
{:/each}
```
```html
<!-- src/components/Item.wisp -->
{@props label: &str, count: i32 = 0}
<button on:click="count++; emit('bump', label)">{:label}: {:count}</button>
```

- Props are browser values (a `{…}` Rust prop is an error);
  `{:...props}` spreads an object (`<Item {:...item} label="x" />`, `label`
  wins). `bind:count` writes back to the parent. `emit('bump', x)` fires the
  parent's `on:bump`; the handler sees `x` as `event`.
- Only text, directives, `{:…}`, client blocks and `{@render children()}`;
  server code in it is a build error. It may render itself (tree view)
  inside an `{:#if}`/`{:#each}` (bare is a build error); depth stops at 64
  in the browser, 32 on the server.
- `setContext(key, value)`/`getContext(key)` share with descendants;
  `const [getUser, setUser] = context()` makes a keyed pair.
- A prop's Rust type matters only where Rust renders the component; a
  browser-only one takes any `Json` type. With `$props()` (no `{@props}`)
  every prop is optional and Rust shows it as the browser would; declare it
  in `{@props}` to use it in Rust (`{#if}`, methods).

```html
<!-- src/components/Pill.wisp -->
<span class={:['pill', tone]} {:...rest}>{:text}</span>
<script>
  let { label: text, tone = 'plain', ...rest } = $props()
</script>
```
`<Pill label="new" tone="warm" title="Just in" />` (`title` goes to `rest`).

## Custom elements

`{@element "x-card"}` first in a component also builds it as a custom
element at `/_app/c/el/x-card.js` (AGENTS.md has the form). Any site:
`<script type="module" src="https://app.example/_app/c/el/x-card.js"></script>`
then `<x-card title="Hi" count="3" featured>Kids</x-card>`. Each prop is an
attribute (`snake_case` as `snake-case`) and property, read as its Rust type
(number; `bool`: present is true, `"false"` false; text; else JSON
`tags='["a"]'`); removing it resets the default. Open shadow root with
scoped `<style>`s; `{@render children()}` is a `<slot>`. Markup is browser
code and defaults are literals (build errors otherwise); the app still
renders `<Card>` server first. Its module sends `access-control-allow-origin:
*` and loads `live.js`, not `wisp.js`.

## State helpers

In any client script, no imports:

```js
watch(() => data.id, (id) => { load(id) })         // when data.id changes
effect(() => { load(id) }, () => [id])             // at start and when id changes (return cleanup)
onMount(() => { ready = true })                    // may return a cleanup
onDestroy(() => socket.close())
setInterval(() => n++, 1000)                       // also setTimeout, requestAnimationFrame,
                                                   // addEventListener: stopped for you
listen('/events', (data) => { last = data })       // server-sent events
await tick()                                       // after the redraw
```

### Shared state

A store outlives components and navigation; put it in `src/lib/`:

```js
// src/lib/cart.js
import { store, persisted, derived } from 'wisp'
export const cart = store([])
export const theme = persisted('theme', 'light')       // localStorage
export const count = derived(() => cart.value.length)
```

`import { cart } from '$lib/cart.js'` in a script, then `cart.value = [...]`
or `$cart`. A store has `.value`, `set(v)`, `update(fn)`, `subscribe(fn)` and
is deep. `src/lib/**/*.js` is served; `'wisp'` and `'$lib/…'` imports work in
them, scripts and `+page.js`.

## Islands
A page ships JS only for files with client code; a component can wait:

```html
<Chart client:visible />                        <!-- near the viewport (200px) -->
<Comments client:idle />                        <!-- browser idle -->
<Filters client:media="(min-width: 800px)" />   <!-- query matches -->
<Menu client:interaction />                     <!-- first pointer, focus or key -->
<Badge client:none />                           <!-- never: no module, no values -->
<section client:visible>{:#each rows as row}…{:/each}</section>   <!-- an element with browser code -->
```

`client:load` (default) starts with the page. The server paints every
island, so it works as HTML until it wakes; the click that wakes one is
replayed. `client:*` on a component needs it to have browser code. Islands
get no `modulepreload`; a page of only islands loads no runtime until one
wakes. A component with no browser code is a server component (no JS);
they nest with islands in any order (`<Panel client:idle><Plain
label="Sales" /></Panel>`, where `Plain.wisp` may hold `<Chart
client:visible />`). An inner island wakes at its own moment and wakes the
waiting island around it (its parent, for `getContext`).

### React, Vue, Svelte, Preact

`wisp add react react-dom react-switch` (framework first), then:

```html
<Island of="react:react-switch" client:visible
  props={:{ checked: on, onChange: (v) => (on = v) }} />
<p>{:on ? 'On' : 'Off'}</p>
<script>
  let on = false
</script>
```

- `of="framework:module"` (`react preact vue svelte`); the default export is
  the component, `#Name` a named one (`react:recharts#LineChart`); `$lib`
  works (`react:$lib/Chart.js#Chart`).
- `props={:…}` is browser (state, callbacks, redraws on change);
  `props={rows}` is Rust, sent once as JSON.
- `client:*` as for any island; children show until it starts. Other
  attributes (`class`, `id`) go on its `<div>`, which morphs leave alone.
- The framework must be in `package.json` (else a build error); it loads
  only on pages with an island of it, one shared copy. A Svelte package
  must ship compiled JS.

### Web components

Import the module, write the tag (Shoelace, Web Awesome `wa-`, Lit). `on:`
takes their events (dashes too); `bind:value` listens for `input`.

```html
<sl-input label="Name" bind:value="name"></sl-input>
<sl-switch on:sl-change="on = event.target.checked">Power</sl-switch>
<script>
  import '@shoelace-style/shoelace/dist/components/input/input.js'
  import '@shoelace-style/shoelace/dist/components/switch/switch.js'
  let on = false
</script>
```

`wisp add @shoelace-style/shoelace`; import each component's own module.
Theme CSS: copy `cdn/themes/light.css` into `static/` and `<link>` it in
`src/app.html`, or `@import` a CDN after `wisp::csp("style-src 'self'
'unsafe-inline' https://cdn.jsdelivr.net")` in `init`. Your own with Lit:
`wisp add lit`, `customElements.define('hello-tag', class extends
LitElement {…})` in a `src/lib` module a script imports. The
`click-events` a11y lint skips custom elements.

## Loading code on demand

`import()` loads when reached:

```html
<button on:click="import('$lib/chart.js').then((m) => m.draw(el))">Chart</button>
```

Resolves like a static import: `$lib/x.js` (or `$lib/x`; `x.js` for
`x.ts`), a relative path into `src/lib`, an npm package. A missing path, or
one outside `src/lib` (the only files the browser loads), is a build error.
No bundle: each lib file, component and package is one immutable hashed
URL shared by all pages. A page `modulepreload`s its static imports all the
way down; `import()` targets and island code wait.

## Morphs and the router

A form action or navigation morphs the page; an instance whose element
survives keeps its state and gets the new `data`. `data-wisp-reset` on an
element around it starts it fresh. Layouts stay mounted.

Same-origin clicks and back/forward fetch and morph, no full reload.
Prefetch on hover (60 ms) and touch (`data-wisp-preload="off"` opts out).
Scroll is restored; focus moves to `[autofocus]`; view transitions when
available. `data-wisp-reload` on a link or parent forces a full load; links
with `target`, `download`, `rel="external"` and `/_app/` are left alone.

```js
goto('/login')                       // or goto(url, { replace: true })
invalidate()                         // run this page's load again
page.value.url.pathname              // page: { url, status, form, state }
navigating.value                     // { from, to } while loading, else null
pushState('?tab=2', { tab: 2 })      // history entry, no navigation
replaceState('', { tab: 3 })         // this entry's state ('' keeps the URL)
```

`pushState(url, state)` (shallow routing, for tabs and modals) adds an entry
at `url` (`''`: this one) and loads nothing; `page.value.state` is reactive
(`{}` on other entries). Back/forward restores it with no request; a reload
keeps it only at its URL. `document` events: `wisp:navigate wisp:update
wisp:goto wisp:refresh wisp:error wisp:push wisp:pop`; forms: `wisp:submit`
(cancelable), `wisp:result`.

### Snapshots

Back, forward and reload restore each changed `<input>`, `<textarea>`,
`<select>` (never passwords, files, hidden, `autocomplete="off"`) from
`sessionStorage`. A script keeps its own state; `snapshot` is the only
export a script may have:

```html
<script>
  let open = false
  export const snapshot = { capture: () => open, restore: (v) => (open = v) }  // capture: any JSON
</script>
```

`data-wisp-noscroll`, `data-wisp-keepfocus`, `data-wisp-replacestate` (on or around a link) keep scroll, keep focus, replace history; `goto(url, { noscroll, keepfocus, replace })` too. Hooks from `'wisp'` return an unsubscribe: `beforeNavigate(({ from, to, pop, cancel }) => ..)`, `afterNavigate`, `onNavigate` (after fetch, before the swap; a returned promise is awaited, a returned function runs after), `preloadData(url)`, `preloadCode(url)`, `invalidateAll()`, `updated.value` (a newer wisp.js or build exists). `cancel()` does not stop back/forward. `+page.js` `load` gets `depends(key)` (a `fetch`ed URL counts); `invalidate('key')` reruns only those loads, no page request.

Phones: pages leave with `pagehide` (back/forward cache; scroll restored). `<body data-wisp-revalidate="30">` refetches data when the tab or network returns (at most every N s, default 30; the morph keeps focus, scroll, typed text). Offline, `<form data-wisp-queue>` (safe to send twice) waits in `sessionStorage`, is sent in order when back, then the page refreshes (`wisp:sent`); other forms show "You are offline" and fire `wisp:result` with `error: "offline"`. Only urlencoded forms queue. A navigation focuses the `<h1>` (else `<main>`) and announces the title; view transitions skip under `prefers-reduced-motion`.

## Forms: `use:enhance`

Plain forms already update in place; `use:enhance` adds hooks:

```html
<form method="post" action="?/add" use:enhance="submit">
  <input name="text" bind:value="text">
  <button disabled={:pending}>Send</button>
</form>
<script>
  let text = '', pending = false
  function submit({ formData, cancel }) {
    pending = true
    return (result) => { pending = false }   // after the page updated
  }
</script>
```

The first function gets `{ form, formData, submitter, action, cancel }`; the
returned one `{ ok, status, location?, data?, error? }`. An action answering
JSON (`Response::json_of(…)`) leaves the form on the page; `data` and
`page.value.form` hold it.

## `+page.js`

Runs in the browser on every navigation, not on the server:
`export async function load({ data, url, params, route, fetch }) { return { ...data, results: await (await fetch('/api/search?q=' + url.searchParams.get('q'))).json() } }`.
The return is the script's `data`. With a server `load` the whole `Data` is
sent (`#[derive(Json)]`). `params.slug` for `blog/[slug]`; `route.id` is
`/blog/[slug]`.

## Server functions

`#[remote]` on a Rust fn in a page block or `src/*.rs` makes it callable from
browser code: no endpoint, fetch or import. In `src/lib`: `import { user }
from 'wisp:remote'`.

```html
---
#[remote]
fn user(id: u64) -> Result<User> {
    USERS.get(id).map(|r| r.value).or_404()
}
---
<button on:click="user(5).then((u) => (name = u.name))">Load</button>
<p>{:name}</p>
<script>
  let name = ''
</script>
```

- POST `/_app/r/<hash>`, arguments a JSON object by name (`{"id":5}`), read
  with `FromJson`; a wrong type or failed `#[validate]` is a 422 by field.
- Answers like an endpoint (`Json` value; `None` is 404; nothing is 204,
  `undefined`). Built like an action (`cx`, `async`, `-> Result` implied);
  same-origin check and `before` run first.
- Errors reject with `status`, `message` (and `errors` for 422);
  `redirect("/x")` navigates.
- `#[remote(get)]`: GET, arguments as JSON in the query (`?id=5&q=%22tea%22`;
  non-JSON text is a string), with an `etag` (304).
- Names are global to browser code: a duplicate, or one JS/Wisp has
  (`fetch`, `goto`), is a build error; a script's own or a page's server
  value of that name wins. `wisp check --types` types each.

## Server rendering off

`const SSR: bool = false;` in the block: the server runs the statements and
sends layouts, `<head>`, the markup as an unpainted `<template>` and the
values it names; the browser draws it. The markup is browser code
(`{:x}`, `{:#each}`): `{…}`, `{#if}`, `{#each}` or a component given `{…}` is
a build error (Rust is fine in `<title>`/`<head>`). For pages that depend on
the browser (size, `localStorage`) or that `+page.js` fills. `wisp build
--spa` serves them from a static host.

## Errors and source maps

An error thrown while a script starts, or in `+page.js`, shows the nearest
`+error.wisp`; handler errors go to the console with file and line. In dev
each browser module has `//# sourceMappingURL=t3.js.map` (also `src/lib`,
`+page.js`), so DevTools shows the `.wisp` file. Release has none unless
`wisp build --sourcemap` (`--static --sourcemap` writes them beside the
modules).

## Installable and offline (PWA)

`src/manifest.json` (`{ "name": "Notes", "theme_color": "#7c3aed", "offline":
true }`) is served at `/manifest.webmanifest` and linked by every page;
nothing is emitted without it. Filled in: `short_name`/`name`, `start_url`
`/`, `display` `standalone`, `icons` from `static/icon*.png` (size read) and
`static/icon*.svg`. One `static/icon.png` (512 px or more) suffices: `wisp
build` also writes 192 and 512 px WebP into `/_app/img/` with cwebp (absent:
a warning, the PNG alone). At startup instead: `wisp::app_manifest(r#"{"name":
"Notes"}"#)?;` in `init`.

`"offline": true` adds Wisp's service worker: at install it keeps `/`, the
browser files and `static/`; pages come from the network and are kept;
offline: the kept page or a 503; a new build drops the old cache. Your own
`src/service-worker.js` (or `.ts`) replaces it: a classic script at
`/service-worker.js`, registered by every page, importing only `'wisp/sw'`
(`env.PUBLIC_X` works):

```js
import { build, files, version } from 'wisp/sw'  // browser files, static/ files, hash of both (empty in dev)
self.addEventListener('install', (e) => {
  e.waitUntil(caches.open(`app-${version}`).then((c) => c.addAll([...build, ...files])))
})
```

The CSP gets `worker-src 'self'` and the registering script's hash.

## Dev: hot reload and devtools

`wisp dev` applies a `.wisp` save with no compile when only browser code or
static text changed: a script or `{:…}` change swaps the file's module
(instances rerun it, keeping `$state` by name, focus, selection and field
values); text alone morphs that file's part between the
`<!--w:src/components/Card.wisp-->` marks; `<style>` swaps the stylesheet.
Else it compiles and morphs. Instances restart (one console line says why)
when the `---` block or `{@props}` changed, the script has a top-level
statement other than declarations, logging, writes to its own names and
instance-ending helpers (`$effect`, `onMount`, `setInterval`…; `init()`,
`if`, `new X()`, `window.x = 1` could run twice), or the swap throws.
Components match by creation order, so a reordering list may trade states.
None of this is in release builds.

`Alt+Shift+W` opens dev-only devtools: component tree with live props and
state (editable), stores, route and server values, timings; "Open" uses
`$WISP_EDITOR` or `$EDITOR`, else `code -g`.
