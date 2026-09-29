---
let greeting = "Hello <server>".to_string();
---
<h1 id="title">{:title}</h1>
<p id="greet" class="card {:mood}">Hi {:name}, you have {:items.length} items.</p>
<button id="exp" :aria-expanded="open" on:click="open = !open">Toggle</button>
<input id="name" bind:value="name">
<button id="add" on:click="add()">Add</button>
<button id="reverse" on:click="items = [...items].reverse()">Reverse</button>
<ul id="list">
  {:#each items as item, i (item.id)}
    <li animate:flip data-id={:item.id}>{:i}: {:item.text}</li>
  {:else}
    <li id="empty">None</li>
  {:/each}
</ul>
{:#if open}
  <p id="open">Open</p>
{:else if name}
  <p id="named">Named {:name}</p>
{:else}
  <p id="closed">Closed</p>
{/if}
<p id="server">{:data.greeting}</p>

<script>
  let title = 'Holes'
  let name = 'Ann'
  let mood = 'happy'
  let open = false
  let items = [{ id: 1, text: 'one' }, { id: 2, text: 'two' }]
  let n = 3
  function add() {
    items = [...items, { id: n, text: 'item ' + n++ }]
  }
</script>
