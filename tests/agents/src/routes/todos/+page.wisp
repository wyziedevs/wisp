---
// src/routes/todos/+page.wisp
struct Data {
    count: usize,
}

fn load() -> Data {
    Data { count: TODOS.len() }
}

#[action]
fn add(todo: Todo) {
    TODOS.add(todo);
}

mod server {
    // DELETE /todos/[id]
    fn delete(id: u64) {
        TODOS.remove(id);
    }
}
---

<p>{count} todos</p>
<form action="?/add" fields />
