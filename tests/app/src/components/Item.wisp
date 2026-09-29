{@props label: &str, count: u32 = 0}
<li class="item">
  <span class="label">{:label}</span>
  <button class="inc" on:click="count++; emit('bump', label)">{:count}</button>
  {@render children()}
  <em class="ctx">{:ctx}</em>
</li>

<script>
  const ctx = getContext('list')
</script>
