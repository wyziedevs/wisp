{@props label: &str}
<div class="chart" data-label={label}>
  <button on:click="n++">{label}: {:n}</button>
</div>

<script>
  let n = 0
</script>
