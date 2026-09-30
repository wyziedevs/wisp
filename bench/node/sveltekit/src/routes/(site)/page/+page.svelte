<script>
    let { data } = $props();
</script>

<svelte:head><title>Roster</title></svelte:head>
<h1>Roster</h1>
<table>
<thead><tr><th>id</th><th>name</th><th>score</th></tr></thead>
<tbody>
{#each data.people as p}
<tr class={p.active ? 'on' : 'off'}><td>{p.id}</td><td>{p.name}</td><td>{p.score}</td></tr>
{/each}
</tbody>
</table>
<form method="post" action="/subscribe"><label>Email <input type="email" name="email" required></label><button>Subscribe</button></form>
