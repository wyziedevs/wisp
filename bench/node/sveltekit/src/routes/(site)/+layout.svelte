<script>
    let { children } = $props();
</script>

<header><nav><a href="/">Home</a><a href="/page">Roster</a><a href="/about">About</a></nav></header>
<main>{@render children()}</main>
<footer><p>Built with the framework under test.</p></footer>
