{@props tone: &str = "neutral"}
<!-- tone: neutral, accent, success, warning, danger. -->
<span class="badge {tone}">{@render children()}</span>

<style>
  .badge {
    display: inline-flex;
    align-items: center;
    padding: 0.125rem 0.5rem;
    border: 1px solid var(--line, #4b4b4b);
    border-radius: 999px;
    color: var(--ink-muted, #a8a8a8);
    font: 600 0.75rem/1.25 var(--font-sans, system-ui, sans-serif);
    white-space: nowrap;
  }
  .accent {
    border-color: var(--accent, #896ce0);
    color: var(--accent-ink, #9f8ce7);
  }
  .success {
    border-color: var(--success, #3e9b4f);
    color: var(--success, #4cc38a);
  }
  .warning {
    border-color: var(--warning, #ad7f19);
    color: var(--warning, #f1a10d);
  }
  .danger {
    border-color: var(--danger, #e5484d);
    color: var(--danger, #ff6369);
  }
</style>
