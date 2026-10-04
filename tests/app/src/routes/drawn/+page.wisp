---
const SSR: bool = false;

let name = "Tea".to_string();
let items = vec![1u32, 2, 3];
---

<title>Drawn {name}</title>
<h1>{:name}</h1>
<ul>{:#each items as i}<li>{:i}</li>{:/each}</ul>
<button on:click="n++">Clicked {:n}</button>
<script>
  let n = 0
</script>
