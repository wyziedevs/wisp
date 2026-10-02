<!-- @feature crud -->
<script>
	import { enhance } from '$app/forms';
	import Details from '$lib/Details.svelte';
	// @feature live
	import { invalidateAll } from '$app/navigation';
	import { onMount } from 'svelte';
	// @feature crud
	let { data } = $props();
	// @feature live
	onMount(() => {
		const events = new EventSource('/posts/events');
		events.onmessage = () => invalidateAll();
		return () => events.close();
	});
</script>

<!-- @feature crud -->
<svelte:head><title>Posts</title></svelte:head>
<a href="/posts/new">New post</a>
{#each data.posts as post (post.id)}
	<Details title={post.title}>
		<p>{post.body}</p>
		<a href="/posts/{post.id}/edit">Edit</a>
		<form method="POST" action="?/delete" use:enhance>
			<button name="id" value={post.id}>Delete</button>
		</form>
	</Details>
{/each}
{#if data.page > 1}<a href="?page={data.page - 1}">Newer</a>{/if}
{#if data.more}<a href="?page={data.page + 1}">Older</a>{/if}
