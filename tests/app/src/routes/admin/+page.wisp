---
// `before` sends visitors who are not signed in to /login, so a `User` is
// always there.
let name = cx.get::<crate::hooks::User>().or_status(401)?.0.clone();
---

<h1>Welcome, {name}</h1>
<!-- What signing in left for this page, the first time it shows. -->
{@flash}
