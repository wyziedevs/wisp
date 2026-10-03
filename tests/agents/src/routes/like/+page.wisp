---
#[action]
fn like(id: u64, email: Email, note: Option<String>, agree: bool, tags: Vec<String>) {
    cx.flash("Liked");                  // cx is added when the body uses it
    redirect("/")                       // 303; or end in `;` to re-render the page
}
---
<form action="?/like">
  <input aria-label="id" name="id">
  <input aria-label="email" type="email" name="email">
  <button>Like</button>
</form>
