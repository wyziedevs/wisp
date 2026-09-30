<!-- @feature list -->
<script>
	let { data } = $props();
</script>

<svelte:head><title>Items</title></svelte:head>
<ul>
	{#each data.items as item}
		<li>{item.name}: ${item.price}</li>
	{/each}
</ul>
