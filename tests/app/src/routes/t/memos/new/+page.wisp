---
fn default(row: Memo) {
    MEMOS.add(row);
    cx.flash("Created");
    redirect("/t/memos")
}
---

<title>New</title>
<form fields="Create" />
