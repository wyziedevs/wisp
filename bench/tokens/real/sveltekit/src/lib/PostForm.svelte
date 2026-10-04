<!-- @feature crud -->
<script>
	import { enhance } from '$app/forms';
	let { form, post = {} } = $props();
</script>

<form method="POST" use:enhance>
	<label>Title <input name="title" required minlength="1" maxlength="100" value={form?.title ?? post.title ?? ''} />
		{#if form?.errors.title}<small class="problem">{form.errors.title}</small>{/if}</label>
	<label>Body <textarea name="body" required value={form?.body ?? post.body ?? ''}></textarea>
		{#if form?.errors.body}<small class="problem">{form.errors.body}</small>{/if}</label>
	<button>Save</button>
</form>
