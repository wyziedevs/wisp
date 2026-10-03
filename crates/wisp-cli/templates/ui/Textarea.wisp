{@props label: &str, name: &str, value: &str = "", rows: u32 = 4, placeholder: Option<&str> = None, hint: Option<&str> = None, problem: Option<&str> = None, required: bool = false, disabled: bool = false}
<!-- hint: help under it. problem: what is wrong with it, shown in place of the hint. -->
<label class="field">
  <span class="label">{label}</span>
  <textarea
    {name}
    {rows}
    {placeholder}
    {required}
    {disabled}
    aria-invalid={problem.map(|_| "true")}
    aria-describedby={problem.or(hint).map(|_| format!("{name}-note"))}>{value}</textarea>
  {#if let Some(note) = problem.or(hint)}<small id="{name}-note" class:problem={problem.is_some()}>{note}</small>{/if}
</label>

<style>
  .field { display: grid; gap: 0.375rem; color: var(--ink, #fafafa); }
  .label { font-size: 0.875rem; font-weight: 600; }
  textarea {
    padding: 0.5rem 0.75rem;
    border: 1px solid var(--line, #4b4b4b);
    border-radius: var(--radius, 0.375rem);
    background: var(--inset, #1a1a1a);
    color: inherit;
    font: inherit;
    resize: vertical;
    transition: border-color var(--change, 200ms);
  }
  textarea::placeholder { color: var(--ink-subtle, #949494); }
  textarea:hover { border-color: var(--line-strong, #6f6f6f); }
  textarea:focus-visible { outline: 2px solid var(--accent, #896ce0); outline-offset: 1px; }
  textarea[aria-invalid="true"] { border-color: var(--danger, #e5484d); }
  textarea:disabled { opacity: 0.5; }
  small { color: var(--ink-muted, #a8a8a8); font-size: 0.8125rem; }
  small.problem { color: var(--danger, #ff6369); }
</style>
