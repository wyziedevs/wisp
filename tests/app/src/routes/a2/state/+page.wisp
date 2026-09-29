<p id="server">{count}</p>
<p id="seen">{:data.count}</p>
<p id="client">{:clicks}</p>
<p id="double">{:double}</p>
<p id="bumps">{:bumps}</p>
<button id="click" on:click="clicks++">Click</button>
<input id="typed" bind:value="typed">
<form method="post" action="?/bump"><button id="bump">Bump</button></form>
<div id="kept"><Tally /></div>
<div id="fresh" data-wisp-reset><Tally /></div>

<script>
  let clicks = 0
  let typed = ''
  let double = $derived(clicks * 2)
  let bumps = 0
  watch(() => data.count, () => bumps++)
</script>
