<p id="size">{:$size}</p>
<p id="double">{:double}</p>
<p id="theme">{:$theme}</p>
<p id="runs">{:runs}</p>
<p id="mounted">{:mounted}</p>
<p id="label">{:title}</p>
<button id="add" on:click="$cart = [...$cart, 'x']">Add</button>
<button id="toggle" on:click="$theme = $theme == 'light' ? 'dark' : 'light'">Theme</button>
<a id="away" href="/a2/nav/one">Away</a>

<script>
  import { cart, theme, size, title } from '$lib/cart.js'
  let runs = 0
  let mounted = false
  let double = $derived($size * 2)
  effect(() => { runs++ }, () => [$size])
  onMount(() => { mounted = true })
</script>
