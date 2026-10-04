---
let items: Vec<String> = (1..=50).map(|i| format!("Item <{i}> & co")).collect();
---

<h1>List</h1>
<ul>
  {#each items as item}
    <li>{item}</li>
  {/each}
</ul>
