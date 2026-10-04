---
use crate::hooks::{Greeting, User};

let greeting = wisp::state::<Greeting>().0;
let user = cx.get::<User>().map(|u| u.0.clone());
---

<h1>{greeting}</h1>
{#if let Some(name) = user}
  <p>Signed in as {name}</p>
{/if}
<Card title={greeting} count={3} featured>
  <Badge label="new" />
  <Badge label={greeting.len()} />
</Card>
<Card title="Plain" />
