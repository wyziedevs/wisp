<nav>
  <a id="one" href="/a2/nav/one" active>One</a>
  <a id="two" href="/a2/nav/two" active>Two</a>
  <a id="reload" data-wisp-reload href="/a2/nav/two">Reload</a>
  <a id="stores" href="/a2/stores">Stores</a>
  <button id="lay" on:click="n++">{:n}</button>
  <span id="url">{:page.value.url.pathname}</span>
</nav>
{@render children()}

<script>
  let n = 0
  setContext('who', 'layout')
</script>
