---
// @feature auth
#[action]
fn logout() {
    cx.sign_out();
    redirect("/login")
}

let user = cx.user(&db::USERS)?;
---
<title>Dashboard</title>
<p>Signed in as {user.email}</p>
<button action="?/logout">Log out</button>
<!-- @feature upload -->
{#if user.avatar.is_some()}<img src="/avatars/{user.id}" alt="Avatar">{/if}
<a href="/avatar">Change avatar</a>
<!-- @feature component -->
<Details title="Account">
  <p>Signed up with {user.email}</p>
</Details>
