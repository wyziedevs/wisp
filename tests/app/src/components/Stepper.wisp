{@props value: i32 = 0, step: i32 = 1}
<button class="step" on:click="value += step">{:value}</button>

<script>
  let { value = $bindable(0), step = 1 } = $props()
</script>
