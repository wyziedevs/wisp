---
// The todo app of the wispweb.dev home page, as written there.
#[model(saved, crud)]
struct Todo {
    #[validate(len = 1..=100)]
    text: String,
}
---

<title>Todos ({TODOS.len()})</title>
<form action="?/add" fields />
{#each TODOS as todo}
  <p>{todo.text} <button action="?/remove&id={todo.id}">Remove</button></p>
{/each}
