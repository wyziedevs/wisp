# Client code

Wisp pages work without JavaScript. When you want a page to react in the
browser, you add a script and a few directives to the same `.wisp` file. No
build step, no bundler, no framework to load.

**The one rule:** braces `{…}` are Rust and run on the server. A quoted value
on a directive, and `{:…}`, are JavaScript and run in the browser.

```html
<button on:click="count++">Clicked {:count} times</button>

<script>
  let count = 0
</script>
```

Turn JavaScript off and the server's HTML is still there. The script only
adds behavior.

## The script

A bare `<script>` (no attributes) is the file's client script. It works in
pages, layouts and components, and runs once for each place the file is shown.

- Top-level `let`s are the state. Assign to one, or change an object,
  array, `Map` or `Set` in it (`todos.push(t)`, `todo.done = true`,
  `seen.add(id)`), and the page updates. A `let` set to a string, number
  or boolean and never assigned again is a plain constant: it costs nothing.
- `let total = $derived(price * qty)` is a value worked out from others.
- At most one script per file.
- `import` lines at the top are moved to the module's head, so
  `import confetti from 'https://esm.sh/canvas-confetti'` works.
- npm packages: `wisp add canvas-confetti` pins it in `package.json`
  (`@1.2.3` or a tag; `wisp remove x`; no Node), then `import confetti
  from 'canvas-confetti'` (also `'pkg/sub'`, `'@scope/pkg'`), in scripts
  and `src/lib`. Dev loads it from esm.sh; `wisp build` downloads it once
  into `.wisp/npm` and the binary serves it from `/_app/c/npm/`: no CDN.
  A package `package.json` lacks, or one with a range (`^1.0`) rather
  than a version, is a build error.
- Errors point at the real `.wisp` file and line (see [Source maps](#source-maps)).

A `<script>` with `type` or `src` stays plain HTML, as before.

### Runes and fine-grained updates

Every read of state in a binding is tracked. A write redraws only the
bindings that read what changed: no virtual DOM, no diffing, and a
component's script runs once, never again on an update. Changes made
together are drawn together, in a microtask.

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

Clicking one item writes one class and one number, nothing else. And
`x === e` in markup, for a state variable `x`, runs again only where the
answer changes: with `class:on="selected === row.id"`, a new `selected`
redraws two rows, not the list. An item's key is tracked too: change it in
place and the item moves.

| Rune | Meaning |
|---|---|
| `let x = $state(v)` | State, deep: objects, arrays, maps and sets track each key. A plain `let x = v` is the same. |
| `$state.raw(v)` | State that changes only when assigned. |
| `$state.snapshot(x)` | A plain copy, for `structuredClone` or a library. |
| `$derived(expr)`, `$derived.by(fn)` | Worked out when read after an input changed. Assigning to it is a build error. |
| `$effect(fn)` | Runs after the DOM is drawn, and again when what it read changes. It may return a cleanup. |
| `$effect.pre(fn)` | The same, before the DOM is drawn. |
| `let { a, b = 1, c: d, ...rest } = $props()` | In a component: its props, with browser defaults (for a prop not given, or `null`); `c: d` reads prop `c` as `d`, `...rest` holds the others. Without `{@props}` this is all a component needs (see [Client components](#client-components)). |
| `$bindable(default)` | A prop a parent may `bind:`. Once a component uses `$props()`, only these can be bound. |
| `$inspect(a, b)` | Logs them as they change. Gone in release builds. |
| `$cart` | For a store `cart`: `cart.value`, tracked. `$cart = x` sets it. |

A class's `$state` fields make its instances state too:

```js
class Todo {
  done = $state(false)
  label = $derived(this.done ? 'done' : 'open')
}
```

`untrack(fn)` reads without tracking. A rune in the wrong place (in markup,
inside a block, misspelled) is a build error at its line.

## TypeScript

`<script lang="ts">` is the client script in TypeScript. So are
`src/lib/*.ts` (`import { f } from '$lib/x'`, no extension needed) and
`+page.ts`.

```html
<script lang="ts">
  import { twice, type Num } from '$lib/util'
  interface Point { x: number; y: number }
  let n: Num = twice(2 as Num)
  const p: Point = { x: 1, y: 2 }
</script>
```

There is no compiler to install: the build strips the types, the way Node's
`--experimental-strip-types` does, and writes spaces where they were, so
every line and column stays (errors and source maps point at the file).
Annotations, `interface`, `type`, `as`, `satisfies`, generics, `!`,
`declare`, `import type`, `abstract`, access modifiers, `implements`,
overloads and optional `?` all go. What TypeScript would turn into code is
a build error that says what to write instead:

| Not erasable | Write |
|---|---|
| `enum Color { Red }` | `const Color = { Red: 'red' } as const` |
| `namespace N { export const x = 1 }` | a module, `src/lib/n.ts` |
| `constructor(private x: number)` | `x: number; constructor(x: number) { this.x = x }` |
| `import fs = require('fs')` | `import fs from 'fs'` |

The build only strips. `wisp check --types` checks the types too, with the
app's own TypeScript (`npm install -D typescript`, or `WISP_TSC` naming a
`tsc`; without Node or it, it says so and skips). Server values are typed
by the Rust compiler, no annotations needed: `let items = vec![Item {..}]`
is `Item[]`, `let n = 3` is `number`, a `#[derive(Json)]` type an
interface. For it the app is built once more (`--features wisp/types`),
and prints the types without running a page. A value whose type has no
TypeScript (a `Json` written by hand) is `unknown`, with a note. Errors
point at the `.wisp` file and line.

## Environment variables

`env.PUBLIC_NAME` in browser code (a script, a directive, `src/lib`,
`+page.js`) is the variable `PUBLIC_NAME`, written in when the app is
built: there is no `env` object in the browser.

```html
<script>
  const r = await fetch(env.PUBLIC_API_URL + '/items')
</script>
```

```sh
# .env, at the app's root (next to Cargo.toml)
PUBLIC_API_URL=https://api.example.com
```

- The values come from the build's environment, and from `.env` for the
  names it lacks (a name given twice is its last value). `wisp dev`
  rebuilds when `.env` changes; a release build has the values it was
  built with.
- Only `PUBLIC_` names reach the browser. `env.DATABASE_URL` in browser
  code is a build error, so a secret can't leak: read it on the server,
  `wisp::env("DATABASE_URL")` (the process's environment, else `.env`'s,
  read when the server starts).
- A `PUBLIC_` name that is not set is a build error, not `undefined` at
  runtime. Set it, even to nothing (`PUBLIC_FLAG=`).
- `env` read whole, or `env[name]`, is an error too: names are filled in
  one at a time. A variable or parameter of your own named `env` is just
  that.

## Translations

`t('cart.items', n)` in a script or directive shows a message of
`src/locales` (see [design.md](design.md#translations)), with no import.
The key is checked at build; the page sends the messages its scripts use,
in its locale, and nothing else. Several values go in an object:
`t('hi', { name, count: n })`. A `t` of your own is just that; `src/lib`
code cannot call it (pass it the text).

## Directives

| Syntax | Meaning |
|---|---|
| `on:click="count++"` | Event handler. A bare name (`on:click="press"`) is called with the event. |
| `bind:value="q"` / `bind:checked="done"` | Two-way binding. `bind:value` alone binds `value`. A name no script, import or server value declares is declared by the binding, as state (`let q`): a live search needs no `<script>`. |
| `bind:group="size"` | Radios (a value) and checkboxes (an array) with one `name`. |
| `bind:files`, `bind:open`, `bind:innerHTML`, `bind:currentTime`, `bind:paused`, … | Any property; the element's own event keeps it current. |
| `bind:clientWidth="w"` | Sizes (`clientWidth/Height`, `offsetWidth/Height`, `contentRect`), from a ResizeObserver. |
| `bind:this="el"` | Puts the element in `el`. |
| `:hidden="!open"` | Live attribute. `false`, `null`, `undefined` remove it. |
| `:text="name"` | Live text. |
| `class:open="isOpen"` | Toggle a class. `class:open` alone reads `open`. |
| `style:--x="x"` | Set a style property. `style:color` alone reads `color`. |
| `class={:['card', { on }]}` | Names from strings, arrays and the keys of objects whose value holds. |
| `style={:{ color, fontSize: '2em' }}` | Properties from an object. |
| `{:...attrs}` | Every key of an object an attribute (an `on…` function a listener). |
| `transition:fade` | Animate in and out: `fade`, `slide`, `scale`, `fly`, `blur`. Takes options: `transition:fly="{ y: 20 }"`. |
| `in:fly` / `out:fade` | Only in, or only out. |
| `transition:spin` | Your function: `spin(el, options, { direction })` returns `{ duration, delay, easing, css: (t, u) => '…' }` or `{ tick(t, u) }`. |
| `use:tip="'Hello'"` | Call `tip(el, 'Hello')`, and its `update` when the value changes. It may return a cleanup function, or `{ update, destroy }`. |
| `use:portal="'#modal'"` | Move the element there (bare: to `<body>`). |
| `animate:flip` | Animate moves in a keyed `{:#each}`. |

```html
<input bind:value="query" on:keydown.enter="search" on:keydown.escape="query = ''">
<ul :hidden="!open" transition:slide>…</ul>
<div class:dark="theme === 'dark'" style:--hue="hue"></div>
```

A bound input starts from what the server rendered, or what the visitor had
already typed before the script started.

### Event modifiers

`.prevent .stop .once .self .capture .passive`, `.window` and `.document` to
listen there, `.outside` (fires for events outside the element),
`.debounce` or `.debounce.300ms` (default 250 ms), key names (`.enter
.escape .space .tab .backspace .delete .up .down .left .right .home .end
.pageup .pagedown`, or a single letter or digit) and `.ctrl .shift .alt
.meta`. An unknown modifier is a build error that lists the valid ones.

```html
<div on:click.outside="open = false">…</div>
<input on:input.debounce.300ms="search()">
<div on:keydown.ctrl.s.prevent.window="save">…</div>
```

## Server values in the browser

Any Rust value your client code mentions is sent to the browser. Only the
parts it mentions.

```html
<script>
  let guess = data.guess          // the script's own `guess`, from the server's
  const total = items.length      // the page's `let items` (or `data.items`)
</script>

{#each keys as key}
  <button on:click="type(key.letter)">{key.letter}</button>   <!-- sends key.letter only -->
{/each}
```

- A page's or layout's Rust names (its block's `let`s, route parameters,
  `Data` fields) by name, or as `data.x.y`. A name the script declares is
  the script's; a browser global (`document`, `location`, `event`, `fetch`…)
  stays the browser's, and its Rust value is `data.location`.
- Props in components.
- Loop, `if let` and `{@const}` values used in a directive.

A live search is then the input and the list:

```html
---
let items = db::items().await;
---
<input bind:value="q" placeholder="Search">
{:#each items.filter((i) => matches(i.name, q)) as item}<p>{:item.name}</p>{:/each}
```

`matches(text, q)` (no import) is whether `text` has `q` in it, whatever
the case; an empty `q` matches everything.

Values are sent as JSON through the `wisp::Json` trait. It is implemented for
numbers, strings, `bool`, `Option`, `Vec`, arrays, tuples and maps. For your
own types, derive it:

```rust
#[derive(Json)]
struct Item { name: String, price: u32 }
```

A value that can't be sent is a compile error that names `wisp::Json`. A
component's prop that is also a script variable is a build error. A
parameter or a local variable of the same name is just that: in
`items.map(data => data.x)` or `function f({ data }) {}`, `data` is not the
page's, and nothing is sent for it.

## `{:expr}` holes

`{:expr}` is a JavaScript expression that stays live anywhere in your markup.

```html
<p>Hi {:name}, you have {:items.length} items.</p>
<p class="card {:mood}" data-id={:item.id}>…</p>
<a href={:url}>Link</a>
```

When the server can work the value out, it writes it too, so the first paint
is right (see [First paint](#first-paint)). Mixing `{…}` and `{:…}` in one
attribute value is a build error.

## Client blocks

Lists and conditions in the browser. Same shape as the server blocks, with a
colon.

```html
{:#if open}
  <p>Open</p>
{:else if name}
  <p>Named {:name}</p>
{:else}
  <p>Closed</p>
{:/if}

<ul>
  {:#each items as item, i (item.id)}
    <li animate:flip>{:i}: {:item.text}</li>
  {:else}
    <li>Nothing here</li>
  {:/each}
</ul>
```

`(item.id)` is the key. Without it, items are matched by position.
`{:#each list}` alone draws its content once per item, and `{:#each 3 as
i}` counts. There is also the `<template each="item, i in list">` and
`<template if="cond">` form.

```html
{:#key user.id}<Profile id={:user.id} />{:/key}      <!-- drawn afresh when it changes -->

{:#await results}
  <p>Loading…</p>
{:then list}
  <p>{:list.length} found</p>
{:catch error}
  <p>{:error.message}</p>
{:/await}
{:#await p then v}…{:/await}                        <!-- no pending branch -->

{:#try}
  <Chart data={:points} />                          <!-- an error drawing it… -->
{:catch error}
  <p>Chart failed: {:error.message}</p>             <!-- …shows this instead -->
  <button on:click="reset()">Retry</button>         <!-- draws the chart again -->
{:/try}
```

The server paints a key block, an await block's pending branch and a try
block's body, as it does the others.

### Special elements

```html
<wisp:window on:keydown.escape="open = false" bind:innerWidth="w" />
<wisp:document on:visibilitychange="save()" bind:visibilityState="seen" />
<wisp:body on:click="menu = false" />
<wisp:element this={:level > 1 ? 'h3' : 'h2'} class="title">{:text}</wisp:element>
```

`<wisp:window>`, `<wisp:document>` and `<wisp:body>` take directives for
that target, and close themselves. `<wisp:element>` takes its tag from
`this`; the server writes it when it knows it.

### Snippets in the browser

`{:@render name(args)}` draws a `{#snippet}` of the file in the browser,
inside a client block or anywhere text goes. The arguments are JavaScript,
and the snippet's parameters are plain names its body reads in `{:…}`:

```html
{#snippet chip(tag)}<b class="chip">{:tag}</b>{/snippet}

{:#each tags as tag (tag)}{:@render chip(tag)}{:/each}
```

Each parameter is a one-item `{:#each}` around the body, so the server
paints it when it knows the argument. The same snippet can still be
rendered on the server with `{@render chip(x)}`.

### First paint

When the server knows what a block, a component, a `{:…}` or a boolean
attribute directive (`:hidden="!open"` with `let open = false`) shows, it
renders it into the page (so no static `hidden` is needed next to it):
people see it before the JavaScript loads, and
without JavaScript at all. The browser then takes those nodes over (no
flicker, nothing drawn twice) and keeps them live. The server knows:

- server values: a page's Rust names (`items`, `data.x`), props, Rust loop values;
- literals: `0`, `'text'`, `true`, `null`, `[1, 2]`, `{ id: 1 }`;
- script variables first set to one of those: `let todos = data.todos`;
- an `{:#each}`'s item and index, inside it;
- `!`, `&&`, `||` and `.length` of those.

Anything else (a call, a sum, a comparison, a `+page.js` page's `data`) is
left to the browser, which draws that part when it starts. If a script
changes a value before it starts, the browser corrects what the server drew.

## Client components

A component inside a client block, or given a `{:…}` value, a `bind:` or an
`on:`, is drawn by the browser.

```html
{:#each names as name (name)}
  <Item label={:name} bind:count="counts[name]" on:bump="bumped = event">
    <b>{:name}!</b>
  </Item>
{:/each}
```

```html
<!-- src/components/Item.wisp -->
{@props label: &str, count: i32 = 0}
<button on:click="count++; emit('bump', label)">{:label}: {:count}</button>
```

- Props are browser values. A `{…}` Rust prop there is an error.
- `{:...props}` gives each key of an object as a prop, where it stands:
  `<Item {:...item} label="x" />` (`label` wins).
- `bind:count` writes back to the parent.
- `emit('bump', x)` fires the parent's `on:bump`; the handler sees `x` as `event`.
- A component drawn in the browser can only use text, directives, `{:…}`,
  client blocks and `{@render children()}`. Server code in it is a build error.
- A component may render itself, as a tree view does, inside an `{:#if}` or
  `{:#each}` that ends; with nothing around it, it is a build error. Nesting
  stops at 64 levels in the browser (and the server paints 32).
- `setContext(key, value)` and `getContext(key)` share values with
  descendants. `const [getUser, setUser] = context()` (in a script or a
  lib module) makes a pair with a key of its own; both work as a script
  starts.

```html
<!-- src/components/Tree.wisp -->
{@props node: &str}
<li>{:node.name}
  <ul>{:#each node.kids as kid (kid.name)}<Tree node={:kid} />{/each}</ul>
</li>
```

The Rust type of a prop only matters where Rust renders the component; a
component only the browser draws can use any `Json` type, such as `&str`.

A component whose props are browser values needs no `{@props}` at all:
`$props()` says what it takes. Each is optional, and any `Json` value; the
server paints a literal default.

```html
<!-- src/components/Pill.wisp -->
<span class={:['pill', tone]} {:...rest}>{:text}</span>

<script>
  let { label: text, tone = 'plain', ...rest } = $props()
</script>
```

```html
<Pill label="new" tone="warm" title="Just in" />      <!-- title goes to rest -->
```

Where Rust renders it, such a prop shows as the browser would show it
(`{label}`, `title={label}`); to work with it in Rust (`{#if}`, a method),
declare it in `{@props}` with its type.

## State helpers

Available inside any client script. No imports.

```js
let double = $derived(count * 2)                   // {:double}, kept current

watch(() => data.id, (id) => { load(id) })         // when data.id changes
effect(() => { load(id) }, () => [id])             // at the start, and when id changes
                                                   // (return a function to clean up)
onMount(() => { ready = true })                    // may return a cleanup
onDestroy(() => socket.close())

setInterval(() => n++, 1000)                       // cleaned up for you
listen('/events', (data) => { last = data })       // server-sent events
await tick()                                       // wait for the redraw
```

`setTimeout`, `setInterval`, `requestAnimationFrame` and `addEventListener`
stop when the component goes away.

### Shared state

A store is a value that outlives one component, and even navigation.
Put it in `src/lib/`:

```js
// src/lib/cart.js
import { store, persisted, derived } from 'wisp'

export const cart = store([])
export const theme = persisted('theme', 'light')    // saved in localStorage
export const count = derived(() => cart.value.length)  // a store's derived value
```

```html
<script>
  import { cart, count } from '$lib/cart.js'
</script>
<button on:click="cart.value = [...cart.value, 'tea']">Add ({:count.value})</button>
```

A store has `.value`, `set(v)`, `update(fn)` and `subscribe(fn)`. It is deep,
like `$state`, and a script can read it as `$cart`. Files in `src/lib/**/*.js` are served with the app; `'wisp'` and
`'$lib/…'` imports work in them, in scripts and in `+page.js`.

## Islands: load only what is needed

A page ships JavaScript only for files with client code. A component can
wait longer: its module is not even downloaded until its moment comes.

```html
<Chart client:visible />                        <!-- scrolled near (200px) -->
<Comments client:idle />                        <!-- when the browser is idle -->
<Filters client:media="(min-width: 800px)" />   <!-- when the query matches -->
<Menu client:interaction />                     <!-- first pointer, focus or key on it -->
<Badge client:none />                           <!-- never: no module, no values sent -->
```

`client:load`, the default, starts with the page. On an element with
browser code, the same waits for the element and what is inside it; its
module is already in, only the work waits:

```html
<section client:visible>{:#each rows as row}…{:/each}</section>
```

 The server paints every
island in full, so until it wakes it reads and works as HTML (links and
forms too). A click that wakes an island is held and replayed once it is
ready. What renders inside an island waits with it.

Why it is faster: the browser parses and runs only the modules for what the
visitor sees or touches. Islands get no `modulepreload`, and a page whose
code is all islands does not load the runtime (`live.js`, about 9 KB) until
the first one wakes. `wisp.js` does the waking.

On a component, `client:*` needs the component to have browser code.

## Server components

A component with no browser code (no script, `{:…}` or directive) is a
server component: the server renders it and it ships no JavaScript. That is
the default. Server components and islands nest in any order, as an
island's children or in its own markup:

```html
<Panel client:idle>                <!-- an island: Panel.wisp has a script -->
  <Plain label="Sales" />          <!-- a server component: no JS -->
</Panel>
```

```html
<!-- src/components/Plain.wisp -->
{@props label: &str}
<div><h3>{label}</h3><Chart client:visible /></div>   <!-- an island again -->
```

Only `Panel`'s and `Chart`'s modules load. An inner island wakes at its own
moment, and wakes the island around it that still waits (its parent, for
`getContext`); what has no `client:*` waits with the island around it.

## React, Vue, Svelte and Preact components

A component from npm, drawn by its own framework, is an `<Island>`:

```sh
wisp add react react-dom react-switch     # the framework first, then the component
```

```html
<Island of="react:react-switch" client:visible
  props={:{ checked: on, onChange: (v) => (on = v) }} />
<p>{:on ? 'On' : 'Off'}</p>

<script>
  let on = false
</script>
```

- `of="framework:module"`: `react`, `preact`, `vue` or `svelte`, then the
  module, whose default export is the component; `#Name` picks a named
  one: `of="react:recharts#LineChart"`. A `$lib` file works too:
  `of="react:$lib/Chart.js#Chart"`.
- `props={:…}` is a browser value: it reads state and holds callbacks, and
  the component draws again when what it reads changes. `props={rows}` is
  Rust instead, sent as JSON once.
- `client:visible`, `client:idle` and the rest wait as for any island;
  without one it starts with the page. Children are shown until it starts
  (`<Island …>Loading…</Island>`). Other attributes (`class`, `id`) go on
  the `<div>` it is drawn in, which page morphs leave alone.
- The framework is added first: `wisp add react react-dom` (`wisp add
  preact`, `wisp add vue`, `wisp add svelte`). It loads only on the pages
  with an island of it, from esm.sh in dev and from the binary after `wisp
  build`, like any npm package; one not in `package.json` is a build
  error. Every package gets the app's version of each framework, so a
  library and the page share one copy of React (its hooks need that).
- The mount for each framework is a few lines added to the page's module:
  React's `createRoot`, Preact's `render`, Vue's `createApp` and Svelte's
  `mount`. A Svelte package must ship compiled JavaScript.

## Web components: Shoelace, Web Awesome, Lit

Custom elements need nothing from Wisp: import the element's module and
write its tag. Directives work on them as on any element: `on:` takes
their own events (dashes too), `bind:value` reads `value` on `input`
events, which their inner inputs send.

```sh
wisp add @shoelace-style/shoelace
```

```html
<sl-input label="Name" bind:value="name"></sl-input>
<sl-switch on:sl-change="on = event.target.checked">Power</sl-switch>
<sl-button variant="primary" on:click="save()">Save {:name}</sl-button>

<script>
  import '@shoelace-style/shoelace/dist/components/input/input.js'
  import '@shoelace-style/shoelace/dist/components/switch/switch.js'
  import '@shoelace-style/shoelace/dist/components/button/button.js'
  let on = false
  function save() {}
</script>
```

- Import each component's own module, as above: the page loads only those.
- Their theme is CSS: copy `cdn/themes/light.css` (or `dark.css`) from the
  package into `static/` and `<link>` it in `src/app.html`, or `@import`
  it from a CDN in `src/app.css` after `wisp::csp("style-src 'self'
  'unsafe-inline' https://cdn.jsdelivr.net")` in `init`.
- Web Awesome, Shoelace's successor, is the same with its package and
  `wa-` tags.
- Your own, with Lit: `wisp add lit`, then a `src/lib` module defines it
  and a script imports it (`import '$lib/hello-tag.js'`):

```js
// src/lib/hello-tag.js
import { LitElement, html } from 'lit'

customElements.define('hello-tag', class extends LitElement {
  static properties = { name: {} }
  render() {
    return html`<button @click=${() => this.dispatchEvent(new CustomEvent('greet', { detail: this.name, bubbles: true }))}>Hello, ${this.name}</button>`
  }
})
```

```html
<hello-tag name="Wisp" on:greet="said = event.detail"></hello-tag>
```

The server sends the tag as written; the browser upgrades it when its module
loads. The `click-events` accessibility lint leaves custom elements alone: their
keyboard is inside them.

## Loading code on demand

`import()` loads a module when the code reaches it, not with the page:

```html
<button on:click="import('$lib/chart.js').then((m) => m.draw(el))">Chart</button>
<script>
  async function edit() {
    const { Editor } = await import('../../lib/editor.js')
  }
</script>
```

It resolves as a static import does: `$lib/x.js` (or `$lib/x`, and
`x.js` for a `x.ts`), a relative path from the file into `src/lib`, an npm
package. A path to no file there is a build error, as is one outside
`src/lib`, whose files are the only ones the browser loads.

There is no bundle to split. Each `src/lib` file, component and npm
package is one module at one URL (immutable, by hash), the same for every
page, so code that two pages import is fetched once and cached for both.
A page `modulepreload`s what its modules import statically, all the way
down (lib files, packages, `extra.js`, components it renders), so the
browser fetches them at once rather than one level at a time. What only an
`import()` names is left until it runs, and so is an island's code.

## Speed

The runtime is two files: `live.js` (about 9 KB compressed), and
`/_app/c/extra.js` (transitions, await and try blocks, components the
browser draws, `bind:group`, persisted stores and the like), which only
the modules that use them import. A keyed list is kept in place with the
fewest moves; events bubble to one listener; a list's template is prepared
once and cloned, and each copy's bindings run as one node. On the
js-framework-benchmark operations (`/a2/bench` in the test app, and
`src/lib/bench.js` to time it) it runs at about 1.2x the time of
hand-written DOM code, where Svelte 5 and Solid run at about 1.1x.

## Morphs keep state

When a form action or a navigation brings in new HTML, Wisp morphs the page.
An instance whose element survives the morph keeps its state and gets the new
`data`. To start fresh, put `data-wisp-reset` on an element around it.

```html
<div data-wisp-reset><Counter /></div>
```

## Router

Links work as links, and Wisp makes them faster. A same-origin click, and the
back and forward buttons, fetch the next page and morph it in, with no full
reload. Layouts stay mounted, so their state survives.

- Prefetch on hover (60 ms) and touch. `data-wisp-preload="off"` opts out.
- Scroll is restored on back and forward. Focus moves to `[autofocus]`.
- View transitions are used when the browser has them.
- `data-wisp-reload` on a link or its parent forces a full load. Links with
  `target`, `download`, `rel="external"`, and `/_app/` are left alone.

Inside scripts:

```js
goto('/login')                       // or goto(url, { replace: true })
invalidate()                         // run this page's load again
matches(item.name, q)                // text has q in it, whatever the case

page.value.url.pathname              // page: { url, status, form, state }
navigating.value                     // { from, to } while loading, else null

pushState('?tab=2', { tab: 2 })      // a history entry: no navigation
replaceState('', { tab: 3 })         // this entry's state ('' keeps the URL)
```

Shallow routing, for tabs and modals: `pushState(url, state)` adds a
history entry at `url` (`''`: this one) and loads nothing; `page.value.state`
is its state, reactive (`{}` on other entries). Back and forward to it
bring the state back with no request. Leaving and coming back loads its URL
and its state; a reload keeps the state only at the URL it was made on.

Events on `document`: `wisp:navigate`, `wisp:update`, `wisp:goto`,
`wisp:refresh`, `wisp:error`, `wisp:push`, `wisp:pop`. On forms: `wisp:submit` (cancelable) and
`wisp:result`.

## Snapshots

What the visitor typed comes back with its history entry: back, forward and
a reload put each field the visitor changed (`<input>`, `<textarea>`,
`<select>`) back, with no code. Passwords, files, hidden fields and anything
under `autocomplete="off"` are never kept. The values live in
`sessionStorage`, for the tab; where it is not there, nothing is kept.

A script keeps its own state the same way:

```html
<script>
  let open = false
  export const snapshot = {
    capture: () => open,              // any JSON, when the entry is left
    restore: (v) => (open = v),       // when it comes back
  }
</script>
```

`snapshot` is the only thing a script may export. It costs a page without
one nothing (it is in `extra.js`).

## Forms: `use:enhance`

Plain forms already update in place. `use:enhance` adds hooks.

```html
<form method="post" action="?/add" use:enhance="submit">
  <input name="text" bind:value="text">
  <button disabled={:pending}>Send</button>
</form>
<ul>
  {#each data.items as item}<li>{item}</li>{/each}
  {:#each optimistic as o}<li>{:o}</li>{:/each}
</ul>

<script>
  let text = '', pending = false, optimistic = []

  function submit({ formData, cancel }) {
    pending = true
    optimistic = [formData.get('text')]
    return (result) => {               // after the page updated
      pending = false
      optimistic = []
    }
  }
</script>
```

The first function gets `{ form, formData, submitter, action, cancel }`. The
one it returns gets `{ ok, status, location?, data?, error? }`. If the action
answers with JSON (`Response::json(…)`), `data` holds it and the form stays on
the page; `page.value.form` has it too.

## `+page.js`

A file beside `+page.wisp` that runs in the browser on every navigation:

```js
// src/routes/search/+page.js
export async function load({ data, url, params, route, fetch }) {
  const r = await fetch('/api/search?q=' + url.searchParams.get('q'))
  return { ...data, results: await r.json() }
}
```

What it returns is the `data` of the page's script. Your `+server.rs`
endpoints are its API. With a server `load`, the whole `Data` is sent, so it
must `#[derive(Json)]`. `params` holds the route's parameters (for
`blog/[slug]`, `params.slug`) and `route.id` its pattern (`/blog/[slug]`),
on the first load and after every navigation. It does not run on the server.

## Server functions

A Rust function marked `#[remote]`, in a page's block or in `src/remote.rs`
(any `src/*.rs`), is a function browser code calls. No endpoint, no fetch,
no import:

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

In `src/lib`, import it: `import { user } from 'wisp:remote'`.

- A call is a POST to `/_app/r/<hash>` with the arguments as a JSON object
  by name (`{"id":5}`). Each is read with `FromJson` (numbers, strings,
  `Option`, `Vec`, a `#[derive(FromJson)]` struct); one that is not the
  type, or fails its `#[validate(…)]`, is a 422 by field, all at once.
- The answer is an endpoint's: a `#[derive(Json)]` value as JSON, `None`
  as a 404, nothing as a 204 (`undefined`).
- It is made as an action is: `cx` when the body uses it, `async` when it
  `.await`s, `-> Result` when it has no `->`. The same-origin check and
  `before` in hooks.rs run first, as for actions.
- An error rejects with an `Error` whose `status` and `message` are the
  server's (and `errors`, by field, for a 422). `redirect("/x")` navigates
  there, as a form's does.
- `#[remote(get)]` makes it a GET, each argument as JSON in the query
  (`?id=5&q=%22tea%22`; text that is not JSON is a string). Its answer has
  an `etag`, so the browser keeps it and gets a 304 when it is the same.

Names are global to browser code: two of one name, or one JavaScript or
Wisp already has (`fetch`, `goto`), is a build error. A name the script
declares itself, or a server value of the page, is that instead.
`wisp check --types` types each (`declare function user(id: number):
Promise<User>`). An app without any serves nothing more and pays nothing.

## Errors

An error thrown while a client script starts, or in `+page.js`, shows the
nearest `+error.wisp`, as a server error would. Errors inside handlers show in
the console with the `.wisp` file and line.

### Source maps

In dev, every browser module ends with `//# sourceMappingURL=t3.js.map`,
served beside it (`/_app/c/t3.js.map`, also for `src/lib` files and
`+page.js`). DevTools then shows the `.wisp` file, and a stack trace its
lines: each line of a script, an import or a directive maps to its line of
the file. A release build has none (no cost), unless built with
`wisp build --sourcemap`; `--static --sourcemap` writes the maps beside
the modules.

## Hot reload that keeps state

Save a `.wisp` file under `wisp dev` and the page changes in place, in
milliseconds, with no compile, whenever only what the browser runs or the
static text changed:

- **Script or `{:…}` markup:** the file's module is swapped in. Each
  instance runs the new script and gets back its `$state` by name (a name
  the script no longer declares is dropped), its effects and handlers are
  the new ones, and its elements are bound again. Focus, the selection and
  what fields hold stay. A component the browser draws keeps its state too.
- **Text alone:** only that page's, layout's or component's part of the
  page is morphed in, between the marks templates write under `wisp dev`
  (`<!--w:src/components/Card.wisp-->`). The rest of the page is untouched.
- **`<style>`:** the stylesheet is swapped, as before.

`wisp dev` knows a change is one of these by compiling the app's Rust in
memory and comparing it, but for text and browser code: the same, a
compile would make the same program, so it sends the new text and modules
to the running app (debug builds take them on `/_wisp/dev/*`, from
loopback, from the same Wisp version) and tells the page. The page's next
load names the new modules, so a reload never gets an old one.

Anything else compiles, and the page then morphs as before, still keeping
state where it can. It starts the instances afresh, and the console says
why in one line, when:

- the `---` block or `{@props}` changed;
- the script has a top-level statement other than declarations, logging,
  writes to its own names and helpers that end with the instance
  (`$effect`, `onMount`, `setInterval`...): `init()`, `if (…)`, `new X()`,
  `window.x = 1` could do twice what they did once;
- the swap throws, which loads the page again.

Release builds have none of this: `live.js` and `wisp.js` minify to the
same bytes.

## Devtools and the component workshop

Both are for `wisp dev` alone: a release build has neither, and its
`live.js` is byte for byte what it was without them.

**Devtools.** Press `Alt+Shift+W` on any page. A panel opens over it, with
no browser extension:

- **Components:** the tree of instances running in the browser. Pick one
  to see its props and its state, live. Numbers, text and booleans are
  edited in place, anything else as JSON; the page redraws as it would for
  its own code. A `$derived` value only shows. `line 12` opens the file at
  the line that declares it.
- **Stores:** each store and `derived` value, named by the line that made
  it (`export const cart = store([])` is `cart`), editable the same way.
- **Route:** the URL, the route and its parameters (sent to pages with a
  `+page.js`), and the server values each page and layout reads.
- **Timings:** the last navigation or action: the whole time, the wait for
  the server, the download, and the morph and redraw.

"Open" sends the file to your editor: `$WISP_EDITOR` or `$EDITOR` (one
with a window; `code`, `cursor` and `codium` get `-g file:line`), else
VS Code's `code -g`, else what the system opens the file with. Only
`localhost` can ask.

A page with no browser code has no runtime to read: the panel shows its
URL and timings.

**Workshop.** `/_wisp/components` lists every component. Beside
`Card.wisp`, a `Card.stories.wisp` holds named examples:

```html
{#story "Featured"}
  <Card featured title="Tea" count={3}>A pot for two.</Card>
{/story}

{#story "Empty"}<Card title="Nothing yet" />{/story}
```

Each story renders on the server, in the app's own shell and CSS, at
`/_wisp/components/Card/featured`. Beside it are controls for the props of
types `&str`, `String`, numbers and `bool`: a text field, a number field
or a checkbox, which render it again as you change them (the URL keeps
them, `?title=Mint`). A component with no stories file gets a "Default"
story from its props' defaults, when every prop it requires is one of
those types; the others say what they need. Saving a stories file
rebuilds; release builds and routing never read them.

## Limits

- The first paint leaves out what it cannot work out (see
  [First paint](#first-paint)); live attributes (`:class`, `class="a {:b}"`)
  keep their static text until the browser starts.
- A hot swap matches a component the browser draws to its state by the
  order they were made, and an instance of the page by its place: a list
  that reorders as it swaps may trade states.
- Deep state tracks plain objects, arrays, maps and sets, and classes with
  `$state` fields. A `Date` or another class's instance is not tracked
  inside: assign it again (`d = d`) to redraw.

## Less Rust boilerplate

Small additions to server templates, in the same spirit.

```html
<p>{count} items</p>                       <!-- a field of Data, no `data.` -->
{#each rows as row}…{/each}

<div class="grid" class:won={data.won}>    <!-- server class toggle -->
<a {href}>Home</a>                         <!-- href={href} -->
<a href="/x" aria-current={current}>       <!-- an Option<_>: left out when None -->
```

```rust
fn get() -> Stats {                        // a +server.rs endpoint:
    Stats { hits: 3 }                      // sent as JSON, #[derive(Json)], no serde
}
```

`data.count` still works.
