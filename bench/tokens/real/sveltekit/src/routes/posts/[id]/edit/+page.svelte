<!-- @feature crud -->
<script>
	import PostForm from '#lib/PostForm.svelte';
	let { data, form } = $props();
</script>

<svelte:head><title>Edit {data.post.title}</title></svelte:head>
<PostForm {form} post={data.post} />
