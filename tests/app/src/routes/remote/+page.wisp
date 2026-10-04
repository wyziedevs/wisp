---
#[remote]
async fn twice(n: u32) -> u32 {
    n * 2
}
---

<h1>Remote</h1>
<button on:click="twice(21).then((v) => (out = v))">Twice</button>
<button on:click="sum()">Sum</button>
<button on:click="missing(7).catch((e) => (out = e.status + ' ' + e.message))">Missing</button>
<button on:click="greet('Ada', 'Dr').then((v) => (out = v))">Greet</button>
<output>{:out}</output>
<script>
  import { addUp } from '$lib/calls.js'

  let out = ''

  async function sum() {
    out = await addUp(2, 3)
  }
</script>
