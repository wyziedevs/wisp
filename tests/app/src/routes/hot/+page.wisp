<h1>Hot page</h1>
<button on:click="count++">Plus one</button>
<output>{:count}</output>
<input bind:value="name">
<script>
  let count = 0
  let name = ''
</script>
