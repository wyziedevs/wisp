<ul id="items">
  {:#each names as name (name)}
    <Item label={:name} bind:count="counts[name]" on:bump="bumped = event"><b class="slot">{:name}!</b></Item>
  {:/each}
</ul>
<p id="bumped">{:bumped}</p>
<p id="total">{:Object.values(counts).reduce((a, b) => a + b, 0)}</p>
<button id="more" on:click="names = [...names, 'c']">More</button>
<button id="reset" on:click="counts = { a: 5, b: 0, c: 0 }">Reset</button>

<script>
  let names = ['a', 'b']
  let counts = { a: 0, b: 0, c: 0 }
  let bumped = ''
  setContext('list', 'ctx-ok')
</script>
