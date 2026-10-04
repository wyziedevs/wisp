<head>
  <title>Snippets</title>
</head>

<!-- A snippet is a template you can pass around. -->
{#snippet plain(name, i)}<b>{:i + 1}.</b> {:name}{/snippet}

<h1>Snippets</h1>

<!-- Pass a named snippet as a prop... -->
<List items={:fruits} row={plain} />

<!-- ...or write one inside the component tag. {:@const} names a value. -->
<List items={:fruits}>
  {#snippet row(name)}
    {:@const loud = name.toUpperCase()}
    <em>{:loud}</em>
  {/snippet}
</List>

<button on:click="fruits = [...fruits, 'pear']">Add a pear</button>

<!-- {:@html} writes markup unescaped: only for markup you trust. -->
<p>{:@html '<i>raw</i> markup'}</p>

<script>
  let fruits = ['plum', 'fig']
</script>
