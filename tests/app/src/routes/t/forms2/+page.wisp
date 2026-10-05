---
// Two forms sharing field names, a box that must be ticked, a radio
// group, a multiple select and a file: what a refused form keeps and
// where each problem goes.
#[action]
fn join(
    #[validate(min_len = 2)] name: String,
    agree: bool,
    kind: String,
    tags: Vec<String>,
    note: String,
    size: Option<String>,
) {
    let _ = (name, kind, tags, note, size);
    if !agree {
        return invalid("agree", "Tick to accept the terms");
    }
    redirect("/t/forms2")
}

#[action]
fn other(name: String, agree: bool, kind: String, note: String) {
    let _ = (agree, kind, note);
    if name == "twice" {
        return Err(Error::invalid("name", "first").and("name", "second"));
    }
    invalid("name", "other is never happy")
}
---

<form action="?/join" id="join" enctype="multipart/form-data">
  <input id="name" aria-label="name" name="name" value="Ada">
  <textarea id="note" aria-label="note" name="note">own</textarea>
  <select id="kind" aria-label="kind" name="kind" value={"b"}><option value="a">A</option><option value="b">B</option></select>
  <select aria-label="tags" name="tags" multiple><option value="x">X</option><option value="y">Y</option><option value="z">Z</option></select>
  <label><input type="radio" name="size" value="s"> S</label>
  <label><input type="radio" name="size" value="m" checked> M</label>
  <label><input type="checkbox" name="agree" id="agree"> I agree</label>
  <input type="file" aria-label="doc" name="doc">
  <button>Join</button>
</form>
<form action="?/other" id="other">
  <input id="name2" aria-label="name2" name="name" value="Bob">
  <textarea aria-label="note2" name="note">other own</textarea>
  <select aria-label="kind2" name="kind" value={"a"}><option value="a">A</option><option value="b">B</option></select>
  <input aria-label="agree2" type="checkbox" name="agree" checked>
  <button>Other</button>
</form>
