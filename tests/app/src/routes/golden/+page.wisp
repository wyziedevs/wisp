---
let start = 2;
let items = vec!["a", "b"];
---
<button on:click="count++" :text="count">{start}</button>
<ul>
  {#each items as item}<li :text="item.toUpperCase()">{item}</li>{/each}
</ul>
<p>{:#each items as it}<b>{:it}</b>{:/each}</p>
<script>
  let count = start
</script>
