---
// @feature search
let items = items().await;
---

<title>Search</title>
<input bind:value="q" placeholder="Search">
<ul>
  {:#each items as item if matches(item.name, q)}
    <li>{:item.name}</li>
  {/each}
</ul>
