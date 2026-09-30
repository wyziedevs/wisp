<!-- @feature form -->
<script>
	import { enhance } from '$app/forms';
	let { form } = $props();
</script>

<svelte:head><title>Contact</title></svelte:head>
<form method="POST" use:enhance>
	<input name="name" value={form?.name ?? ''} />
	{#if form?.errors?.name}<p>{form.errors.name}</p>{/if}
	<input name="email" value={form?.email ?? ''} />
	{#if form?.errors?.email}<p>{form.errors.email}</p>{/if}
	<button>Send</button>
</form>
