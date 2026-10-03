{@props label: &str, name: &str, hint: Option<&str> = None, required: bool = false, disabled: bool = false}
<!-- The native select: its <option>s are the children, so the keyboard, the screen reader
     and the phone's own picker all work. -->
<label class="field">
  <span class="label">{label}</span>
  <select {name} {required} {disabled} aria-describedby={hint.map(|_| format!("{name}-note"))}>{@render children()}</select>
  {#if let Some(hint) = hint}<small id="{name}-note">{hint}</small>{/if}
</label>

<style>
  .field { display: grid; gap: 0.375rem; color: var(--ink, #fafafa); }
  .label { font-size: 0.875rem; font-weight: 600; }
  select {
    min-height: 2.5rem;
    padding: 0 2rem 0 0.75rem;
    border: 1px solid var(--line, #4b4b4b);
    border-radius: var(--radius, 0.375rem);
    background: var(--inset, #1a1a1a) url("data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' width='12' height='12' fill='none' stroke='%23a8a8a8' stroke-width='1.5'%3E%3Cpath d='m3 4.5 3 3 3-3'/%3E%3C/svg%3E") no-repeat right 0.75rem center;
    color: inherit;
    font: inherit;
    appearance: none;
    transition: border-color var(--change, 200ms);
  }
  select:hover { border-color: var(--line-strong, #6f6f6f); }
  select:focus-visible { outline: 2px solid var(--accent, #896ce0); outline-offset: 1px; }
  select:disabled { opacity: 0.5; }
  small { color: var(--ink-muted, #a8a8a8); font-size: 0.8125rem; }
</style>
