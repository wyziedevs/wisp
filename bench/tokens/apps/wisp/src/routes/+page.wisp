<!-- @feature list -->
<title>Items</title>
<ul>
  {#each items().await as item}
    <li>{item.name}: ${item.price}</li>
  {/each}
</ul>
