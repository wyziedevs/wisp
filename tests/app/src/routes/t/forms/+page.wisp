---
// An edit form: a struct taken whole and shown back, and every field that
// does not pass listed at once, each by its input.
#[derive(FromJson, Clone)]
struct Post {
    #[validate(len = 1..=20)]
    title: String,
    #[validate(min_len = 3)]
    body: String,
    kind: String,
    stars: u8,
    draft: bool,
    note: Option<String>,
}

static SAVED: Shared<Option<Post>> = Shared::new(None);

#[action]
fn default(post: Post) {
    *SAVED.lock() = Some(post);
    redirect("/t/forms")
}

#[action]
fn pair(#[validate(len = 2..)] a: String, #[validate(min = 5)] n: u8, pw: String) {
    let _ = (a, n, pw);
    redirect("/t/forms")
}

let post = SAVED.lock().clone().unwrap_or(Post {
    title: "First".into(),
    body: "Hello".into(),
    kind: "b".into(),
    stars: 3,
    draft: false,
    note: None,
});
---
<form method="post">
  <input name="title" value={post.title}>
  <textarea name="body">{post.body}</textarea>
  <select name="kind" value={post.kind}><option value="a">A</option><option value="b">B</option></select>
  <input name="stars" value={post.stars}>
  <input name="note" value={post.note}>
  <input name="draft" type="checkbox" checked={post.draft}>
</form>
<form action="?/pair">
  <input name="a"><input name="n" type="number"><input name="pw" type="password">
  <p id="n-problem">{cx.problem("n")}</p>
</form>
