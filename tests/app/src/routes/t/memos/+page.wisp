---
fn remove(id: u64) {
    MEMOS.remove(id);
}

let rows = MEMOS.page(cx, 20);
---

<title>Memos</title>

{@flash}
<a href="/t/memos/new">New</a>
{#each rows as row}
  <p>
    <a href="/t/memos/{row.id}/edit">{row.title}</a>
    <button action="?/remove&id={row.id}">Delete</button>
  </p>
{:else}
  <p>None yet.</p>
{/each}
{@pager rows}
