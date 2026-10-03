<!-- @feature component -->
{@props title}
<section>
  <button on:click="open = !open">{title}</button>
  <div :hidden="!open"><slot /></div>
</section>
<script>let open = false</script>
