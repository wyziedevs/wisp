<wisp:head><title>Fortunes</title></wisp:head>
<table>
  <tr><th>id</th><th>message</th></tr>
  {#each data.fortunes as f}
    <tr><td>{f.id}</td><td>{f.message}</td></tr>
  {/each}
</table>
