---
/// The pages `wisp build --static` writes for this route.
fn entries() -> Vec<&'static str> {
    vec!["hello", "second-post"]
}
---
<h1>Post {slug}</h1>
