{@props inner: bool = false}
<section class="panel">
  <button class="outer" on:click="n++">Outer {:n}</button>
  {#if inner}<Plain label="direct" />{/if}
  <slot />
</section>

<script>
  let n = 0
</script>
