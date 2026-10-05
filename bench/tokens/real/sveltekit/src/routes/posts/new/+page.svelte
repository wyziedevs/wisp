<!-- @feature crud -->
<script>
	import PostForm from '#lib/PostForm.svelte';
	let { form } = $props();
</script>

<svelte:head><title>New post</title></svelte:head>
<PostForm {form} />
