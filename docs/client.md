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

- Top-level `let`s are the state. Assign to one and the page updates.
- At most one per file.
- `import` lines at the top are moved to the module's head, so
  `import confetti from 'https://esm.sh/canvas-confetti'` works.
- Errors point at the real `.wisp` file and line.

A `<script>` with `type` or `src` stays plain HTML, as before.

## Directives

| Syntax | Meaning |
|---|---|
| `on:click="count++"` | Event handler. A bare name (`on:click="press"`) is called with the event. |
| `bind:value="q"` / `bind:checked="done"` | Two-way binding. |
| `bind:this="el"` | Puts the element in `el`. |
| `:hidden="!open"` | Live attribute. `false`, `null`, `undefined` remove it. |
| `:text="name"` | Live text. |
| `class:open="isOpen"` | Toggle a class. |
| `style:--x="x"` | Set a style property. |
| `transition:fade` | Animate in and out: `fade`, `slide`, `scale`, `fly`. Takes options: `transition:fly="{ y: 20 }"`. |
| `use:tip="'Hello'"` | Call `tip(el, 'Hello')`. It may return a cleanup function, or `{ update, destroy }`. |
| `animate:flip` | Animate moves in a keyed `{:#each}`. |

```html
<input bind:value="query" on:keydown.enter="search" on:keydown.escape="query = ''">
<ul :hidden="!open" transition:slide hidden>…</ul>
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
  let guess = data.guess          // data.* comes from `load`
  const total = data.items.length
</script>

{#each keys as key}
  <button on:click="type(key.letter)">{key.letter}</button>   <!-- sends key.letter only -->
{/each}
```

- `data.x.y` in pages and layouts.
- Props in components.
- Loop, `if let` and `{@const}` values used in a directive.

Values are sent as JSON through the `wisp::Json` trait. It is implemented for
numbers, strings, `bool`, `Option`, `Vec`, arrays, tuples and maps. For your
own types, derive it:

```rust
#[derive(Json)]
pub struct Item { pub name: String, pub price: u32 }
```

A value that can't be sent is a compile error that names `wisp::Json`. A
name that is both a Rust value and a script variable is a build error.

## `{:expr}` holes

`{:expr}` is a JavaScript expression that stays live anywhere in your markup.

```html
<p>Hi {:name}, you have {:items.length} items.</p>
<p class="card {:mood}" data-id={:item.id}>…</p>
<a href={:url}>Link</a>
```

When the expression is a plain server value (`{:data.count}`), the server
also writes it, so the first paint is right. Mixing `{…}` and `{:…}` in one
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

`(item.id)` is the key. Without it, items are matched by position. There is
also the `<template each="item, i in list">` and `<template if="cond">`
form.

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
- `bind:count` writes back to the parent.
- `emit('bump', x)` fires the parent's `on:bump`; the handler sees `x` as `event`.
- A component drawn in the browser can only use text, directives, `{:…}`,
  client blocks and `{@render children()}`. Server code in it is a build error.
- A component that renders itself is a build error.
- `setContext(key, value)` and `getContext(key)` share values with descendants.

## State helpers

Available inside any client script. No imports.

```js
const double = derived(() => count * 2)            // {:double.value}

effect(() => { document.title = `(${count})` })    // after every redraw
effect(() => { load(id) }, () => [id])             // only when id changes
                                                   // (return a function to clean up)
onMount(() => { ready = true })                    // may return a cleanup
onDestroy(() => socket.close())

setInterval(() => n++, 1000)                       // cleaned up for you
listen('/events', (data) => { last = data })       // server-sent events
await tick()                                       // wait for the redraw
```

`setTimeout`, `setInterval`, `requestAnimationFrame` and `addEventListener`
redraw after each callback and stop when the component goes away.

### Shared state

A store is a value that outlives one component, and even navigation.
Put it in `src/lib/`:

```js
// src/lib/cart.js
import { store, persisted, derived } from 'wisp'

export const cart = store([])
export const theme = persisted('theme', 'light')    // saved in localStorage
export const count = derived(() => cart.value.length)
```

```html
<script>
  import { cart, count } from '$lib/cart.js'
</script>
<button on:click="cart.value = [...cart.value, 'tea']">Add ({:count.value})</button>
```

A store has `.value`, `set(v)`, `update(fn)` and `subscribe(fn)`. Setting it
redraws. Files in `src/lib/**/*.js` are served with the app; `'wisp'` and
`'$lib/…'` imports work in them, in scripts and in `+page.js`.

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

page.value.url.pathname              // page: { url, status, form }
navigating.value                     // { from, to } while loading, else null
```

Events on `document`: `wisp:navigate`, `wisp:update`, `wisp:goto`,
`wisp:refresh`, `wisp:error`. On forms: `wisp:submit` (cancelable) and
`wisp:result`.

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
export async function load({ data, url, fetch }) {
  const r = await fetch('/api/search?q=' + url.searchParams.get('q'))
  return { ...data, results: await r.json() }
}
```

What it returns is the `data` of the page's script. Your `+server.rs`
endpoints are its API. With a server `load`, the whole `Data` is sent, so it
must `#[derive(Json)]`. It gets `url`, not `params`, and it does not run on
the server.

## Errors

An error thrown while a client script starts, or in `+page.js`, shows the
nearest `+error.wisp`, as a server error would. Errors inside handlers show in
the console with the `.wisp` file and line.

## Limits

- Client blocks and client components have no server first paint. The
  browser draws them.
- Editing a client script rebuilds; editing markup still hot-swaps.

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
pub fn get() -> Response {
    Response::json_of(&Stats { hits: 3 })  // uses #[derive(Json)], no serde
}
```

`data.count` still works.
