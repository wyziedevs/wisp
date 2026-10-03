{@props id: &str, labels: &[&str]}
<!-- One tab per label; the children are the panels, one element each, in the same order:
     <Tabs id="drinks" labels={["Tea", "Coffee"]}><div>…</div><div>…</div></Tabs>.
     Arrow keys, Home and End move between tabs; Tab goes into the panel. -->
<div class="tabs" bind:this="root">
  <div class="list" role="tablist" on:click="click" on:keydown="key">
    {#each labels as label, i}
      <button
        type="button"
        role="tab"
        id="{id}-tab-{i}"
        aria-controls="{id}-panel-{i}"
        aria-selected={if i == 0 { "true" } else { "false" }}
        tabindex={if i == 0 { "0" } else { "-1" }}>{label}</button>
    {/each}
  </div>
  <div class="panels">{@render children()}</div>
</div>

<script>
  let root
  const tabs = () => [...root.querySelector('[role="tablist"]').children]

  function show(to) {
    tabs().forEach((tab, i) => {
      tab.setAttribute('aria-selected', i === to)
      tab.tabIndex = i === to ? 0 : -1
    })
    ;[...root.querySelector('.panels').children].forEach((panel, i) => (panel.hidden = i !== to))
  }

  function click(event) {
    const tab = event.target.closest('[role="tab"]')
    if (tab) show(tabs().indexOf(tab))
  }

  function key(event) {
    const all = tabs()
    const at = all.indexOf(document.activeElement)
    const to = { ArrowRight: at + 1, ArrowLeft: at - 1, Home: 0, End: all.length - 1 }[event.key]
    if (at < 0 || to === undefined) return
    event.preventDefault()
    const next = (to + all.length) % all.length
    show(next)
    all[next].focus()
  }

  onMount(() => {
    ;[...root.querySelector('.panels').children].forEach((panel, i) => {
      panel.id = `${id}-panel-${i}`
      panel.role = 'tabpanel'
      panel.tabIndex = 0
      panel.setAttribute('aria-labelledby', `${id}-tab-${i}`)
    })
    show(0)
    root.classList.add('ready')
  })
</script>

<style>
  .list { display: flex; gap: 0.25rem; border-bottom: 1px solid var(--line, #4b4b4b); }
  [role="tab"] {
    margin-bottom: -1px;
    padding: 0.625rem 0.875rem;
    border: 0;
    border-bottom: 2px solid transparent;
    background: none;
    color: var(--ink-muted, #a8a8a8);
    font: 600 0.9375rem/1 var(--font-sans, system-ui, sans-serif);
    cursor: pointer;
    transition: color var(--change, 200ms), border-color var(--change, 200ms);
  }
  [role="tab"]:hover { color: var(--ink, #fafafa); }
  [role="tab"][aria-selected="true"] { border-bottom-color: var(--accent, #896ce0); color: var(--ink, #fafafa); }
  [role="tab"]:focus-visible { outline: 2px solid var(--accent, #896ce0); outline-offset: -2px; }
  .panels { padding: 1rem 0; }
  .panels > :global([role="tabpanel"]:focus-visible) { outline: 2px solid var(--accent, #896ce0); outline-offset: 2px; }
  .tabs:not(.ready) .panels > :global(:not(:first-child)) { display: none; }
</style>
