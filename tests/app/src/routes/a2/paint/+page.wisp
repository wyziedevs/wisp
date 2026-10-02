<h1 id="title">{:title}</h1>
<ul id="todos">
  {:#each todos as todo, i (todo.id)}
    <li class="todo" class:done="todo.done">{:i}. {:todo.text}{:#if todo.done} <b class="tick">done</b>{/if}</li>
  {:else}
    <li id="none">none</li>
  {/each}
</ul>
<ul id="empty">{:#each data.empty as n}<li>{:n}</li>{:else}<li id="empty-else">empty</li>{/each}</ul>
<ul id="tree"><Tree node={:data.tree} /></ul>
<ul id="cards">
  {:#each todos as todo (todo.id)}
    <Item label={:todo.text} count={:todo.id}><i class="slot">{:todo.id}</i></Item>
  {/each}
</ul>
{#each data.todos as t}
  <p class="rust">{:#if t.done}<span class="rdone">{t.text}</span>{/if}</p>
{/each}
<p id="shadow">{:[1, 2].map(data => data * 2).join()}-{:first({ data: 'local' })}</p>
<button id="add" on:click="todos = [...todos, { id: todos.length + 1, text: 'new', done: false }]">Add</button>
<button id="flip" on:click="todos = todos.map((t) => ({ ...t, done: !t.done }))">Flip</button>
<a id="to-params" href="/a2/params/one">Params</a>
<div id="menu" :hidden="!open">menu</div><details id="more" :open="open" :hidden="shown > 1">x</details>

<script>
  let todos = data.todos
  let title = data.title
  let open = false
  let shown = 0
  function first({ data }) {
    return data
  }
  setContext('list', 'paint')
</script>
