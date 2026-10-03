<!-- @feature crud -->
<script>
	import { enhance } from '$app/forms';
	let { form, post = {} } = $props();
</script>

<form method="POST" use:enhance>
	<input name="title" value={form?.title ?? post.title ?? ''} />
	{#if form?.errors.title}<p>{form.errors.title}</p>{/if}
	<textarea name="body" value={form?.body ?? post.body ?? ''}></textarea>
	{#if form?.errors.body}<p>{form.errors.body}</p>{/if}
	<button>Save</button>
</form>
