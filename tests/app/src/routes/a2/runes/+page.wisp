<p id="count">{:count}</p>
<p id="double">{:double}</p>
<p id="done">{:done} of {:todos.length} done</p>
<ul id="todos">
  {:#each todos as todo (todo.id)}
    <li class:done="todo.done" on:click="todo.done = !todo.done">{:todo.text}</li>
  {:/each}
</ul>
<button id="inc" on:click="count++">+1</button>
<button id="push" on:click="todos.push({ id: todos.length + 1, text: 'new', done: false })">Push</button>
<p id="cart">{:$cart.length}</p>
<Stepper bind:value="count" step={:2} />

<script>
  import { cart } from '$lib/cart.js'
  let count = $state(0)
  let todos = $state([{ id: 1, text: 'one', done: false }, { id: 2, text: 'two', done: true }])
  let double = $derived(count * 2)
  let done = $derived(todos.filter((t) => t.done).length)
  $effect(() => {
    document.title = `Runes ${count}`
  })
  $inspect(count)
</script>
