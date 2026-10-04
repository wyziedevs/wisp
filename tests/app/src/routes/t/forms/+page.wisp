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
  <input aria-label="title" name="title" value={post.title}>
  <textarea aria-label="body" name="body">{post.body}</textarea>
  <select aria-label="kind" name="kind" value={post.kind}><option value="a">A</option><option value="b">B</option></select>
  <input aria-label="stars" name="stars" value={post.stars}>
  <input aria-label="note" name="note" value={post.note}>
  <input aria-label="draft" name="draft" type="checkbox" checked={post.draft}>
</form>
<form action="?/pair">
  <input aria-label="a" name="a"><input aria-label="n" name="n" type="number"><input aria-label="pw" name="pw" type="password">
  <p id="n-problem">{cx.problem("n")}</p>
</form>
