{@props node: &str = ""}
<li class="node">{:node.name}
  {:#if node.kids && node.kids.length}
    <ul>
      {:#each node.kids as kid (kid.name)}
        <Tree node={:kid} />
      {/each}
    </ul>
  {/if}
</li>
