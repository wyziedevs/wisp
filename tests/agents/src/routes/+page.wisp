---
static TODOS: Table<String> = Table::new();

#[action]
fn add(#[validate(len = 1..=100)] text: String) {
    TODOS.add(text);
}

#[action]
fn remove(id: u64) {
    TODOS.remove(id);
}

let count = TODOS.len();
---
<title>Todos ({count})</title>
<form action="?/add">
  <input name="text">
</form>
{#each TODOS.all() as todo}
  <p>{todo} <button action="?/remove&id={todo.id}">x</button></p>
{/each}
