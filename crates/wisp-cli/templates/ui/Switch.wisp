{@props label: &str, name: &str, checked: bool = false, disabled: bool = false}
<!-- A checkbox with role="switch": Space toggles it, a form sends name=on when it is on. -->
<label class="switch">
  <input type="checkbox" role="switch" {name} {checked} {disabled}>
  <span>{label}</span>
</label>

<style>
  .switch { display: inline-flex; align-items: center; gap: 0.625rem; color: var(--ink, #fafafa); cursor: pointer; }
  input {
    position: relative;
    width: 2.25rem;
    height: 1.25rem;
    margin: 0;
    border: 1px solid var(--line-strong, #6f6f6f);
    border-radius: 999px;
    background: var(--inset, #1a1a1a);
    appearance: none;
    cursor: inherit;
    transition: background-color var(--change, 200ms), border-color var(--change, 200ms);
  }
  input::before {
    content: "";
    position: absolute;
    top: 2px;
    left: 2px;
    width: 0.875rem;
    height: 0.875rem;
    border-radius: 50%;
    background: var(--ink-muted, #a8a8a8);
    transition: transform var(--change, 200ms) var(--ease-out, ease-out), background-color var(--change, 200ms);
  }
  input:checked { border-color: var(--accent, #896ce0); background: var(--accent, #896ce0); }
  input:checked::before { background: var(--on-accent, #141414); transform: translateX(1rem); }
  input:focus-visible { outline: 2px solid var(--accent, #896ce0); outline-offset: 2px; }
  input:disabled, input:disabled + span { opacity: 0.5; cursor: not-allowed; }
</style>
