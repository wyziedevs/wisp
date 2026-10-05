<script>
  const items = Array.from({ length: 1000 }, (_, i) => `Item <${i + 1}> & co`);
</script>

<h1>List</h1>
<ul>
  {#each items as item}
    <li>{item}</li>
  {/each}
</ul>
