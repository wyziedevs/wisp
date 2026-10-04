{@props fallback: &str = ""}
<!-- Draws its children only in the browser: the server sends the fallback, `onMount` swaps them in. -->
{:#if ready}{@render children()}{:else}<span class="client-only">{:fallback}</span>{:/if}

<script>
  let ready = false
  onMount(() => { ready = true })
</script>
