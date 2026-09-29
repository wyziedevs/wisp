{@props label: &str}
<div class="dropdown">
  <button on:click="open = !open" :aria-expanded="open" :text="title">{label}</button>
  <div :hidden="!open" transition:fade hidden>Menu</div>
</div>

<script>
  let open = false
  const title = label + '!'
</script>
