<!-- @feature upload -->
<script>
	import { enhance } from '$app/forms';
	let { form } = $props();
</script>

<svelte:head><title>Avatar</title></svelte:head>
<form method="POST" enctype="multipart/form-data" use:enhance>
	<label>Avatar <input name="avatar" type="file" required accept="image/*" />
		{#if form?.error}<small class="problem">{form.error}</small>{/if}</label>
	<button>Upload</button>
</form>
