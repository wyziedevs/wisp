---
use std::sync::atomic::{AtomicU64, Ordering};

static RENDERS: AtomicU64 = AtomicU64::new(0);

let n = RENDERS.fetch_add(1, Ordering::Relaxed);
---
<h1>Shallow</h1>
<p id="n">{n}</p>
<button id="two" on:click="pushState('?tab=2', { tab: 2 })">Tab two</button>
<button id="three" on:click="replaceState('', { tab: 3 })">Tab three</button>
<output>{:page.value.state.tab ?? 1}</output>
<a id="away" href="/a2/islands">Away</a>
<a id="save" href="/a2/download">Save</a>
<p id="nav">{:navigating.value ? 'going' : 'here'}</p>
