---
#[derive(Json)]
struct User {
    name: String,
}

let user = User { name: "Ada".into() };
---
<button on:click="count++">Clicked {:count} times</button>
<script>
  let count = 0                       // top-level lets are state
  let big = $derived(count > 5)
  let name = user.name                // a block's `let user` (or `data.user`)
</script>
<Card title="Counter" featured>{user.name}</Card>
