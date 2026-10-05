---
use std::sync::atomic::{AtomicU32, Ordering};

static COUNT: AtomicU32 = AtomicU32::new(0);

#[action]
fn bump() {
    COUNT.fetch_add(1, Ordering::Relaxed);
}

let count = COUNT.load(Ordering::Relaxed);
let lit = 5;
let on = false;
let who = "me";
let list = vec![1, 2];
---

<p id="server">{count}</p>
<p id="seen">{:data.count}</p>
<p id="client">{:clicks}</p>
<p id="double">{:double}</p>
<p id="bumps">{:bumps}</p>
<button id="click" on:click="clicks++">Click</button>
<input id="typed" bind:value="typed">
<p id="lit">{:lit}</p>
<p id="on">{:on ? "on" : "off"}</p>
<p id="who">{:who}</p>
<p id="list">{:list.length}</p>
<button id="lit-up" on:click="lit++">Lit</button>
<button id="toggle" on:click="on = !on">Toggle</button>
<button id="add" on:click="list = [...list, 3]">Add</button>
<input id="who-in" bind:value="who">
<form method="post" action="?/bump"><button id="bump">Bump</button></form>
<div id="kept"><Tally /></div>
<div id="fresh" data-wisp-reset><Tally /></div>

<script>
  let clicks = 0
  let typed = ''
  let double = $derived(clicks * 2)
  let bumps = 0
  watch(() => data.count, () => bumps++)
</script>
