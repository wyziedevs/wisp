<!-- @feature auth -->
<script>
	import { enhance } from '$app/forms';
	let { form } = $props();
</script>

<svelte:head><title>Log in</title></svelte:head>
<form method="POST" use:enhance>
	<label>Email <input name="email" type="email" required value={form?.email ?? ''} />
		{#if form?.error}<small class="problem">{form.error}</small>{/if}</label>
	<label>Password <input name="password" type="password" /></label>
	<button>Log in</button>
</form>
