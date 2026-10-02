<!-- @feature upload -->
<script>
	import { enhance } from '$app/forms';
	let { form } = $props();
</script>

<svelte:head><title>Avatar</title></svelte:head>
<form method="POST" enctype="multipart/form-data" use:enhance>
	<input name="avatar" type="file" accept="image/*" />
	{#if form?.error}<p>{form.error}</p>{/if}
	<button>Upload</button>
</form>
