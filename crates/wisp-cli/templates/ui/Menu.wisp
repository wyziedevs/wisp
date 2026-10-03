{@props id: &str, label: &str}
<!-- A menu button: the list is a popover (Escape and a click outside close it, no script for
     that). Its children are the items: <a role="menuitem" href="/x"> or <button role="menuitem">.
     Arrow keys, Home and End move between them; choosing one closes the menu. -->
<div class="menu">
  <button
    class="trigger"
    type="button"
    popovertarget={id}
    aria-haspopup="menu"
    on:keydown="key"
    style="anchor-name: --{id}">{label}</button>
  <div
    {id}
    class="list"
    popover
    role="menu"
    aria-label={label}
    style="position-anchor: --{id}"
    bind:this="list"
    on:click="pick"
    on:keydown="key">
    {@render children()}
  </div>
</div>

<script>
  let list
  const items = () => [...list.querySelectorAll('[role="menuitem"]:not([disabled])')]

  function key(event) {
    const all = items()
    const at = all.indexOf(document.activeElement)
    const open = list.matches(':popover-open')
    const to = { ArrowDown: at + 1, ArrowUp: at < 0 ? -1 : at - 1, Home: 0, End: -1 }[event.key]
    if (to === undefined || !all.length) return
    event.preventDefault()
    if (!open) list.showPopover()
    all.at(to % all.length).focus()
  }

  function pick(event) {
    if (event.target.closest('[role="menuitem"]')) list.hidePopover()
  }

  onMount(() => {
    list.addEventListener('toggle', (event) => {
      if (event.newState === 'open') items()[0]?.focus()
      else if (list.contains(document.activeElement)) list.previousElementSibling.focus()
    })
  })
</script>

<style>
  .menu { display: inline-block; }
  .trigger {
    min-height: 2.5rem;
    padding: 0 1rem;
    border: 1px solid var(--line, #4b4b4b);
    border-radius: var(--radius, 0.375rem);
    background: var(--panel, #1f1f1f);
    color: var(--ink, #fafafa);
    font: 600 0.9375rem/1 var(--font-sans, system-ui, sans-serif);
    cursor: pointer;
  }
  .trigger:hover { background: var(--panel-hover, #2e2e2e); }
  .trigger:focus-visible { outline: 2px solid var(--accent, #896ce0); outline-offset: 2px; }
  .list {
    min-width: 10rem;
    margin: 0.25rem 0;
    padding: 0.25rem;
    border: 1px solid var(--line, #4b4b4b);
    border-radius: var(--radius, 0.375rem);
    background: var(--panel, #1f1f1f);
    color: var(--ink, #fafafa);
    box-shadow: 0 0.75rem 2rem rgb(0 0 0 / 0.4);
  }
  @supports (position-area: block-end) {
    .list { inset: auto; position-area: block-end span-inline-end; position-try-fallbacks: flip-block; }
  }
  .list:popover-open { display: grid; }
  .list > :global([role="menuitem"]) {
    padding: 0.5rem 0.75rem;
    border: 0;
    border-radius: calc(var(--radius, 0.375rem) * 0.66);
    background: none;
    color: inherit;
    font: inherit;
    text-align: start;
    text-decoration: none;
    cursor: pointer;
  }
  .list > :global([role="menuitem"]:hover), .list > :global([role="menuitem"]:focus-visible) {
    outline: none;
    background: var(--panel-hover, #2e2e2e);
  }
</style>
