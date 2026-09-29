---
#[action]
fn default(email: String) -> Result {
    if !email.contains('@') {
        return invalid("email", "needs an @");
    }
    redirect("/")
}

let price = shop::price("tea");
---
<head><title>Sign up</title></head>
<form method="post"><input name="email" value={cx.input("email")}></form>
{#if let Some(p) = cx.problem("email")}<p class="problem">{p}</p>{/if}
<p id="price">{price} at {cx.path()}</p>
