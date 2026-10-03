{@props message: &str = ""}
<!-- Put one in the layout. It shows message (say, a flash: let flash = cx.flashed(); then
     <Toast message={flash.unwrap_or_default()} />), and what any script sends it:
     dispatchEvent(new CustomEvent('toast', { detail: 'Saved' })). Screen readers announce each
     one (role="status"); each goes after 5 seconds, or at its close button. -->
<wisp:window on:toast="add(event.detail)" />
<div class="toasts" role="status">
  {:#each toasts as toast (toast.id)}
    <p class="toast" transition:fly="{ y: 16 }">
      <span>{:toast.text}</span>
      <button type="button" aria-label="Dismiss" on:click="drop(toast.id)">×</button>
    </p>
  {:/each}
</div>

<script>
  let toasts = []
  let next = 0

  function add(text) {
    const id = next++
    toasts.push({ id, text: String(text) })
    setTimeout(() => drop(id), 5000)
  }

  function drop(id) {
    toasts = toasts.filter((t) => t.id !== id)
  }

  onMount(() => {
    if (message) add(message)
  })
</script>

<style>
  .toasts {
    position: fixed;
    right: 1rem;
    bottom: 1rem;
    z-index: 100;
    display: grid;
    gap: 0.5rem;
    width: min(22rem, calc(100vw - 2rem));
  }
  .toast {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 0.75rem;
    margin: 0;
    padding: 0.75rem 0.75rem 0.75rem 1rem;
    border: 1px solid var(--line, #4b4b4b);
    border-left: 3px solid var(--accent, #896ce0);
    border-radius: var(--radius, 0.375rem);
    background: var(--panel, #1f1f1f);
    color: var(--ink, #fafafa);
    box-shadow: 0 0.75rem 2rem rgb(0 0 0 / 0.4);
  }
  button {
    width: 1.75rem;
    height: 1.75rem;
    border: 0;
    border-radius: var(--radius, 0.375rem);
    background: none;
    color: var(--ink-muted, #a8a8a8);
    font-size: 1.125rem;
    cursor: pointer;
  }
  button:hover {
    background: var(--panel-hover, #2e2e2e);
    color: var(--ink, #fafafa);
  }
  button:focus-visible {
    outline: 2px solid var(--accent, #896ce0);
    outline-offset: 1px;
  }
</style>
