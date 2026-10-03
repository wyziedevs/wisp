{@props title: &str, open: bool = false, group: Option<&str> = None}
<!-- The native <details>: Enter and Space toggle it, no script. Items with the same group
     open one at a time. -->
<details class="item" {open} name={group}>
  <summary>{title}</summary>
  <div class="body">{@render children()}</div>
</details>

<style>
  .item { border-bottom: 1px solid var(--line, #4b4b4b); color: var(--ink, #fafafa); }
  summary {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 1rem;
    padding: 0.875rem 0.25rem;
    font-weight: 600;
    list-style: none;
    cursor: pointer;
  }
  summary::-webkit-details-marker { display: none; }
  summary::after {
    content: "";
    width: 0.5rem;
    height: 0.5rem;
    border: solid var(--ink-muted, #a8a8a8);
    border-width: 0 1.5px 1.5px 0;
    transform: translateY(-25%) rotate(45deg);
    transition: transform var(--change, 200ms) var(--ease-out, ease-out);
  }
  .item[open] > summary::after { transform: translateY(25%) rotate(-135deg); }
  summary:focus-visible { outline: 2px solid var(--accent, #896ce0); outline-offset: 2px; }
  .body { padding: 0 0.25rem 1rem; color: var(--ink-muted, #a8a8a8); }
</style>
