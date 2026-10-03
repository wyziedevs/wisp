<!-- @feature auth -->
<script>
	import { enhance } from '$app/forms';
	let { form } = $props();
</script>

<svelte:head><title>Log in</title></svelte:head>
<form method="POST" use:enhance>
	<input name="email" type="email" value={form?.email ?? ''} />
	<input name="password" type="password" />
	{#if form?.error}<p>{form.error}</p>{/if}
	<button>Log in</button>
</form>
