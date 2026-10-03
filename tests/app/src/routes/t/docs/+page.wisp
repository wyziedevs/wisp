---
#[action]
fn add(doc: Upload) {
    cx.flash(&format!("{} {} {}", doc.name, doc.size, doc));
    redirect("/t/docs")
}
---
<form action="?/add" fields></form>
