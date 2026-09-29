---
// `before` sends visitors who are not signed in to /login, so a `User` is
// always there.
let name = cx.get::<crate::hooks::User>().or_status(401)?.0.clone();
// What signing in left for this page, the first time it shows.
let hello = cx.flashed();
---
<h1>Welcome, {name}</h1>
{#if let Some(hello) = hello}
  <p class="flash">{hello}</p>
{/if}
