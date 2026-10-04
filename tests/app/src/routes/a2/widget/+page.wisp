<h1>Widget</h1>
<div id="w" data-wisp-keep use:widget="{ label }"></div>
<input id="label" bind:value="label">

<script>
  let label = 'hello'
  // mount(el, props) / update / destroy of a bundle your own esbuild or vite
  // wrote into static/ (here: static/widget.js).
  function widget(el, props) {
    let w
    import('/widget.js').then((m) => (w = m.mount(el, props)))
    return { update: (p) => w?.update(p), destroy: () => w?.destroy() }
  }
</script>
