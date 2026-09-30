---
// @feature search
let items = db::items().await;
---
<title>Search</title>
<input bind:value="q" placeholder="Search">
<ul>
  {:#each data.items.filter((i) => i.name.toLowerCase().includes(q.toLowerCase())) as item}
    <li>{:item.name}</li>
  {:/each}
</ul>
