{@props id: &str, text: &str}
<!-- Shows text over its child on hover and on keyboard focus; Escape hides it. The child's
     first focusable element is described by it (aria-describedby), so screen readers read it. -->
<span
  class="tooltip"
  class:quiet="quiet"
  bind:this="root"
  on:keydown.escape="quiet = true"
  on:focusout="quiet = false"
  on:pointerleave="quiet = false">
  {@render children()}
  <span {id} class="tip" role="tooltip">{text}</span>
</span>

<script>
  let quiet = false
  let root
  onMount(() => {
    root.querySelector('a[href], button, input, select, textarea, [tabindex]')?.setAttribute('aria-describedby', id)
  })
</script>

<style>
  .tooltip {
    position: relative;
    display: inline-block;
  }
  .tip {
    position: absolute;
    bottom: calc(100% + 0.375rem);
    left: 50%;
    z-index: 10;
    width: max-content;
    max-width: 16rem;
    padding: 0.375rem 0.625rem;
    border: 1px solid var(--line, #4b4b4b);
    border-radius: var(--radius, 0.375rem);
    background: var(--panel, #1f1f1f);
    color: var(--ink, #fafafa);
    font-size: 0.8125rem;
    line-height: 1.4;
    pointer-events: none;
    opacity: 0;
    visibility: hidden;
    transform: translate(-50%, 0.25rem);
    transition: opacity var(--change, 200ms), transform var(--change, 200ms) var(--ease-out, ease-out), visibility var(--change, 200ms);
  }
  .tooltip:hover > .tip, .tooltip:focus-within > .tip {
    opacity: 1;
    visibility: visible;
    transform: translate(-50%, 0);
  }
  .tooltip.quiet > .tip {
    opacity: 0;
    visibility: hidden;
  }
</style>
