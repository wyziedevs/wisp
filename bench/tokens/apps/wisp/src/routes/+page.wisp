---
// @feature list
let items = db::items().await;
---
<title>Items</title>
<ul>
  {#each items as item}
    <li>{item.name}: ${item.price}</li>
  {/each}
</ul>
