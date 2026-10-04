{@props items: Vec<String>, row: Snippet<&String, usize>}
<ul>
  {:#each items as item, i}
    <li>{:@render row(item, i)}</li>
  {:/each}
</ul>
