{@props label: &str, name: &str, value: &str = "on", checked: bool = false, disabled: bool = false}
<label class="check">
  <input type="checkbox" {name} {value} {checked} {disabled}>
  <span>{label}</span>
</label>

<style>
  .check { display: inline-flex; align-items: center; gap: 0.5rem; color: var(--ink, #fafafa); cursor: pointer; }
  input {
    display: grid;
    place-content: center;
    width: 1.125rem;
    height: 1.125rem;
    margin: 0;
    border: 1px solid var(--line-strong, #6f6f6f);
    border-radius: calc(var(--radius, 0.375rem) * 0.66);
    background: var(--inset, #1a1a1a);
    appearance: none;
    cursor: inherit;
    transition: background-color var(--change, 200ms), border-color var(--change, 200ms);
  }
  input::before {
    content: "";
    width: 0.625rem;
    height: 0.625rem;
    background: var(--on-accent, #141414);
    clip-path: polygon(14% 44%, 0 65%, 50% 100%, 100% 16%, 80% 0%, 43% 62%);
    transform: scale(0);
    transition: transform var(--change, 200ms) var(--ease-out, ease-out);
  }
  input:checked { border-color: var(--accent, #896ce0); background: var(--accent, #896ce0); }
  input:checked::before { transform: scale(1); }
  input:focus-visible { outline: 2px solid var(--accent, #896ce0); outline-offset: 2px; }
  input:disabled, input:disabled + span { opacity: 0.5; cursor: not-allowed; }
</style>
