---
use std::sync::atomic::{AtomicU32, Ordering};

static BUMPS: AtomicU32 = AtomicU32::new(0);

#[action]
fn bump() {
    BUMPS.fetch_add(1, Ordering::Relaxed);
}

let clicks = 5;
let on = false;
let who = "me";
let list = vec![1, 2];
let bumps = BUMPS.load(Ordering::Relaxed);
---

<p id="clicks">{:clicks}</p>
<p id="on">{:on ? "on" : "off"}</p>
<p id="who">{:who}</p>
<p id="list">{:list.length}</p>
<p id="bumps">{:bumps}</p>
<button id="click" on:click="clicks++">Click</button>
<button id="toggle" on:click="on = !on">Toggle</button>
<button id="add" on:click="list = [...list, 3]">Add</button>
<button id="local" on:click="bumps += 10">Local</button>
<input id="who-in" bind:value="who">
<form method="post" action="?/bump"><button id="bump">Bump</button></form>
