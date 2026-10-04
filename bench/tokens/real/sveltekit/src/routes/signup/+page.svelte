<!-- @feature auth -->
<script>
	import { enhance } from '$app/forms';
	let { form } = $props();
</script>

<svelte:head><title>Sign up</title></svelte:head>
<form method="POST" use:enhance>
	<label>Email <input name="email" type="email" required value={form?.email ?? ''} />
		{#if form?.errors.email}<small class="problem">{form.errors.email}</small>{/if}</label>
	<label>Password <input name="password" type="password" required minlength="8" />
		{#if form?.errors.password}<small class="problem">{form.errors.password}</small>{/if}</label>
	<button>Sign up</button>
</form>
