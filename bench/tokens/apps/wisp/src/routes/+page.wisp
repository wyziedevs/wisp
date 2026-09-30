<!-- @feature list -->
<title>Items</title>
<ul>
  {#each db::items().await as item}
    <li>{item.name}: ${item.price}</li>
  {/each}
</ul>
