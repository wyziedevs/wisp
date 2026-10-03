<h1 :text="heading">{title}</h1>
<button on:click="open = !open" class:active="open" :aria-expanded="open">Menu</button>
<ul :hidden="!open">
  {#each items as item, i}<!-- wisp-ignore a11y-click-events -->
    <li on:click.prevent="pick(item.name)" class:picked="picked === item.name">{i}: {item.name}</li>
  {/each}
</ul>
<input aria-label="field" bind:value="query">
<p :text="`Looking for ${query}`"></p>
<template each="note, n in notes"><p :text="n + ': ' + note"></p></template>
<Dropdown label="More" />

<script>
  let open = false
  let picked = null
  let query = ''
  const heading = data.title.toUpperCase()
  const notes = ['a', 'b']

  function pick(name) {
    picked = name
  }
</script>
