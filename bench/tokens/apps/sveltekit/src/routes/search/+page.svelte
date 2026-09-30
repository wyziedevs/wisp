<!-- @feature search -->
<script>
	let { data } = $props();
	let q = $state('');
	let found = $derived(data.items.filter((i) => i.name.toLowerCase().includes(q.toLowerCase())));
</script>

<svelte:head><title>Search</title></svelte:head>
<input bind:value={q} placeholder="Search" />
<ul>
	{#each found as item}
		<li>{item.name}</li>
	{/each}
</ul>
