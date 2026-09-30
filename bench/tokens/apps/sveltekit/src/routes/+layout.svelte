<!-- @feature layout -->
<script>
	let { children } = $props();
</script>

<nav>
	<a href="/">Items</a>
	<a href="/contact">Contact</a>
	<a href="/search">Search</a>
</nav>
{@render children()}
