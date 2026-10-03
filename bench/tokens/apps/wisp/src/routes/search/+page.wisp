---
// @feature search
let items = items().await;
---
<title>Search</title>
<input aria-label="Search" bind:value="q" placeholder="Search">
<ul>
  {:#each items.filter((i) => matches(i.name, q)) as item}
    <li>{:item.name}</li>
  {:/each}
</ul>
