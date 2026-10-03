{@props variant: &str = "primary", size: &str = "md", kind: &str = "button", href: Option<&str> = None, disabled: bool = false, label: Option<&str> = None}
<!-- variant: primary, secondary, ghost, danger. size: sm, md, lg. kind: button, submit, reset.
     With href it is a link that looks like a button. label: the accessible name, for an icon alone. -->
{#if let Some(href) = href}
  <a class="button {variant} {size}" {href} aria-label={label}>{@render children()}</a>
{:else}
  <button class="button {variant} {size}" type={kind} {disabled} aria-label={label}>{@render children()}</button>
{/if}

<style>
  .button {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    gap: 0.5em;
    min-height: 2.5rem;
    padding: 0 1rem;
    border: 1px solid transparent;
    border-radius: var(--radius, 0.375rem);
    font: 600 0.9375rem/1 var(--font-sans, system-ui, sans-serif);
    text-decoration: none;
    cursor: pointer;
    transition: background-color var(--change, 200ms), border-color var(--change, 200ms);
  }
  .sm {
    min-height: 2rem;
    padding: 0 0.75rem;
    font-size: 0.875rem;
  }
  .lg {
    min-height: 3rem;
    padding: 0 1.5rem;
    font-size: 1rem;
  }
  .primary {
    background: var(--accent, #896ce0);
    color: var(--on-accent, #141414);
  }
  .primary:hover {
    background: var(--accent-hover, #9f8ce7);
  }
  .secondary {
    border-color: var(--line, #4b4b4b);
    background: var(--panel, #1f1f1f);
    color: var(--ink, #fafafa);
  }
  .secondary:hover {
    border-color: var(--line-strong, #6f6f6f);
    background: var(--panel-hover, #2e2e2e);
  }
  .ghost {
    background: transparent;
    color: var(--ink, #fafafa);
  }
  .ghost:hover {
    background: var(--panel-hover, #2e2e2e);
  }
  .danger {
    background: var(--danger, #e5484d);
    color: #fff;
  }
  .danger:hover {
    background: color-mix(in oklab, var(--danger, #e5484d) 85%, #fff);
  }
  .button:focus-visible {
    outline: 2px solid var(--accent, #896ce0);
    outline-offset: 2px;
  }
  .button:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }
  @media (pointer: coarse) {
    .button {
      min-height: 2.75rem;
    }
  }
</style>
