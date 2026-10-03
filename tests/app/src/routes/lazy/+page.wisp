<h1>Lazy</h1>
<button on:click="import('$lib/names.js').then((m) => (out = m.label('lazy')))">Load</button>
<button on:click="sum()">Sum</button>
<output>{:out}</output>
<script>
  import { addUp } from '$lib/calls.js'

  let out = ''

  // Loaded on the first click, not with the page.
  async function sum() {
    const { addUp: again } = await import('../../lib/calls.js')
    out = String((await addUp(1, 1)) + (await again(1, 2)))
  }
</script>
