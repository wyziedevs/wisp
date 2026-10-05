---
#[model(saved, crud)] // Json + FromJson + Clone, all pub; `pub static TODOS: Table<Todo> = Table::saved("todos");`
struct Todo {         // and this page's actions add(todo: Todo), remove(id: u64), update(id: u64, todo: Todo)
    #[validate(len = 1..=100)]
    text: String,
}
---

<title>Todos ({TODOS.len()})</title>
<form action="?/add" fields />
{#each TODOS as todo}
  <p>{todo.text} <button action="?/remove&id={todo.id}">Remove</button></p>
{/each}
