<script>
    let { data } = $props();
</script>

<svelte:head><title>Fortunes</title></svelte:head>
<table><tbody>
<tr><th>id</th><th>message</th></tr>
{#each data.fortunes as f}
<tr><td>{f.id}</td><td>{f.message}</td></tr>
{/each}
</tbody></table>
