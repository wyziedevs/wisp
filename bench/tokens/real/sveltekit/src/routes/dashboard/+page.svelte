<!-- @feature auth -->
<script>
	import { enhance } from '$app/forms';
	import Details from '#lib/Details.svelte';
	let { data } = $props();
</script>

<svelte:head><title>Dashboard</title></svelte:head>
<p>Signed in as {data.email}</p>
<form method="POST" action="?/logout" use:enhance><button>Log out</button></form>
<!-- @feature upload -->
{#if data.avatar}<img src="/avatars/{data.id}" alt="Avatar" />{/if}
<a href="/avatar">Change avatar</a>
<!-- @feature component -->
<Details title="Account">
	<p>Signed up with {data.email}</p>
</Details>
