---
#[action]
fn add(todo: Todo) {
    TODOS.add(todo);
}

let count = TODOS.len();
---

<title>Todos ({count})</title>
<form action="?/add" fields><button>Add</button></form>
{#each TODOS.all() as todo}
  <p>{todo.text}</p>
{/each}
