<!-- @feature auth -->
<script>
	import { enhance } from '$app/forms';
	let { form } = $props();
</script>

<svelte:head><title>Sign up</title></svelte:head>
<form method="POST" use:enhance>
	<input name="email" type="email" value={form?.email ?? ''} />
	{#if form?.errors.email}<p>{form.errors.email}</p>{/if}
	<input name="password" type="password" />
	{#if form?.errors.password}<p>{form.errors.password}</p>{/if}
	<button>Sign up</button>
</form>
