<p id="size">{:size.value}</p>
<p id="double">{:double.value}</p>
<p id="theme">{:theme.value}</p>
<p id="runs">{:runs}</p>
<p id="mounted">{:mounted}</p>
<p id="label">{:title}</p>
<button id="add" on:click="cart.value = [...cart.value, 'x']">Add</button>
<button id="toggle" on:click="theme.value = theme.value == 'light' ? 'dark' : 'light'">Theme</button>
<a id="away" href="/a2/nav/one">Away</a>

<script>
  import { cart, theme, size, title } from '$lib/cart.js'
  let runs = 0
  let mounted = false
  const double = derived(() => size.value * 2)
  effect(() => { runs++ }, () => [size.value])
  onMount(() => { mounted = true })
</script>
