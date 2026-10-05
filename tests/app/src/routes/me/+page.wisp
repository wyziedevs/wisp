---

#[action]
fn avatar(#[validate(max_size = 64 * KB)] avatar: Image) {
    PEOPLE.update(cx.signed_in()?, |p| p.avatar = Some(avatar));
}

#[action]
fn leave() {
    cx.sign_out();
    redirect("/")
}

#[action]
fn everywhere() {
    wisp::sign_out_everywhere(cx.signed_in()?)?;
    redirect("/")
}

let me = cx.user()?;
---

<h1>{me.name}</h1>
{#if me.avatar.is_some()}
  <img src="/avatars/{me.id}" alt="">
{/if}
<form action="?/avatar">
  <input aria-label="avatar" type="file" name="avatar">
  <small class="problem">{cx.problem("avatar")}</small>
</form>
<button action="?/leave">Leave</button>
