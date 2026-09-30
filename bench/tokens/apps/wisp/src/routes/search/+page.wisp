---
// @feature search
let items = db::items().await;
---
<title>Search</title>
<input bind:value="q" placeholder="Search">
<ul>
  {:#each items.filter((i) => matches(i.name, q)) as item}
    <li>{:item.name}</li>
  {:/each}
</ul>
