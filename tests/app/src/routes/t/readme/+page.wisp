---
let count: i64 = cx.cookie_or("count", 0);

#[action]
fn add(by: i64) {
    let count: i64 = cx.cookie_or("count", 0);
    cx.set_cookie("count", count + by);
}
---
<h1>Clicked {count} times</h1>

<form action="?/add">
  <button name="by" value="1" disabled={count >= 10}>Click me</button>
</form>
