<span class={:['pill', tone, { on }]} {:...rest}>{:text}</span>

<script>
  let { label: text, tone = 'plain', on = false, ...rest } = $props()
</script>
