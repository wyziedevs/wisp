<!-- @feature component -->
<script>
	let { title, children } = $props();
	let open = $state(false);
</script>

<section>
	<button onclick={() => (open = !open)}>{title}</button>
	{#if open}{@render children()}{/if}
</section>
