---
let items: Vec<(&str, u32)> = vec![("pen", 2), ("ink", 5)];
---
<script>
  let tags = ['x', 'y'];
</script>
{#snippet cell(name, qty)}<td>{name}</td><td>{qty}</td>{/snippet}
{#snippet line(item: &(&str, u32), i: usize)}<td>{i}</td>{@render cell(item.0, item.1)}{/snippet}
<table id="own">{#each items as it, i}<tr>{@render line(it, i)}</tr>{/each}</table>
<Table rows={items} row={line} />
<Table rows={items}>
  {#snippet row(r, i)}<td>{i}:{r.0}</td>{/snippet}
  <caption>kids</caption>
</Table>
{#snippet chip(label)}<b class="chip">{:label}</b>{/snippet}
<p id="chips">{:#each tags as tag}{:@render chip(tag)}{:/each}</p>
