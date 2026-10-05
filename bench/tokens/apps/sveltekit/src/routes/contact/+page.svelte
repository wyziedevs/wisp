<!-- @feature form -->
<script>
	import { enhance } from '$app/forms';
	let { form } = $props();
</script>

<svelte:head><title>Contact</title></svelte:head>
<form method="POST" use:enhance>
	<label>Name <input name="name" required minlength="1" pattern="[\s\S]{0,50}" value={form?.name ?? ''} />
		{#if form?.errors?.name}<small class="problem">{form.errors.name}</small>{/if}</label>
	<label>Email <input name="email" type="email" required value={form?.email ?? ''} />
		{#if form?.errors?.email}<small class="problem">{form.errors.email}</small>{/if}</label>
	<button>Send</button>
</form>
