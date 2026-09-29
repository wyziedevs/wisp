---
static TODOS: Table<String> = Table::new();

#[action]
fn add(#[validate(len = 1..=10)] text: String) {
    TODOS.add(text);
}

#[action]
fn remove(id: u64) {
    TODOS.remove(id);
}
---
<title>Todos {TODOS.len()}</title>
<form action="?/add">
  <input name="text">
  <input name="secret" type="password">
  <button>Add</button>
  <p class="problem">{cx.problem("text")}</p>
</form>
<ul>
  {#each TODOS.all() as todo}
    <li>{todo}<button action="?/remove&id={todo.id}">x</button></li>
  {/each}
</ul>
<Box>boxed</Box>
<svg><title>icon</title></svg>
