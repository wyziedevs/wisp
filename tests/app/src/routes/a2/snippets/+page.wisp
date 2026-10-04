<head>
  <title>Snippets</title>
</head>

{#snippet plain(name, i)}<b>{:i + 1}.</b> {:name}{/snippet}

<h1>Snippets</h1>

<List items={:fruits} row={plain} />

<List items={:fruits}>
  {#snippet row(name)}
    {:@const loud = name.toUpperCase()}
    <em>{:loud}</em>
  {/snippet}
</List>

<button on:click="fruits = [...fruits, 'pear']">Add a pear</button>
<div id="note">{:@html '<i>raw</i> markup'}</div>

<script>
  let fruits = ['plum', 'fig']
</script>
