---
// The whole route in one file: load, an action and an
// endpoint (`mod server`). No +page.rs, no +server.rs.

let count = *COUNT.lock();

#[action]
fn add(by: i64) {
    *COUNT.lock() += by;
}

mod server {
    fn delete() {
        *COUNT.lock() = 0;
    }
}
---

<title description="A page, a form and an endpoint in one file">Counter</title>

<div class="text-column">
  <h1>Count: {count}</h1>
  <form method="post" action="?/add" use:enhance>
    <button name="by" value="1">+1</button>
    <button name="by" value="-1">-1</button>
  </form>
  <p>This route is one <code>.wisp</code> file: <code>curl -X DELETE /counter</code> resets it.</p>
</div>
