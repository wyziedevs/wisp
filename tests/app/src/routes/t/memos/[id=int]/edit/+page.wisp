---
fn default(id: u64, row: Memo) {
    MEMOS.set(id, row).or_404()?;
    cx.flash("Saved");
    redirect("/t/memos")
}

let row = MEMOS.get(id).or_404()?;
---

<title>Edit {row.title}</title>
<form fields={row} />
