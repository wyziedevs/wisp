{@props id: &str, title: &str, trigger: &str = "Open", close: &str = "Close"}
<!-- The native modal <dialog>, opened by its trigger button (commandfor, no script): focus
     moves in and comes back, Escape and the Close button close it, the page behind is inert.
     Another button opens it too: <button commandfor="the-id" command="show-modal">. -->
<button class="trigger" type="button" commandfor={id} command="show-modal">{trigger}</button>
<dialog {id} aria-labelledby="{id}-title" closedby="any">
  <h2 id="{id}-title">{title}</h2>
  <div class="body">{@render children()}</div>
  <form method="dialog"><button class="close">{close}</button></form>
</dialog>

<style>
  button {
    min-height: 2.5rem;
    padding: 0 1rem;
    border: 1px solid var(--line, #4b4b4b);
    border-radius: var(--radius, 0.375rem);
    background: var(--panel, #1f1f1f);
    color: var(--ink, #fafafa);
    font: 600 0.9375rem/1 var(--font-sans, system-ui, sans-serif);
    cursor: pointer;
  }
  @media (pointer: coarse) {
    button {
      min-height: 2.75rem;
    }
  }
  button:hover {
    border-color: var(--line-strong, #6f6f6f);
    background: var(--panel-hover, #2e2e2e);
  }
  button:focus-visible {
    outline: 2px solid var(--accent, #896ce0);
    outline-offset: 2px;
  }
  dialog {
    width: min(32rem, calc(100vw - 2rem));
    padding: 1.5rem;
    border: 1px solid var(--line, #4b4b4b);
    border-radius: calc(var(--radius, 0.375rem) * 2);
    background: var(--panel, #1f1f1f);
    color: var(--ink, #fafafa);
    box-shadow: 0 1.5rem 4rem rgb(0 0 0 / 0.5);
  }
  dialog[open] {
    animation: rise var(--change, 200ms) var(--ease-out, ease-out);
  }
  dialog::backdrop {
    background: rgb(0 0 0 / 0.6);
  }
  h2 {
    margin: 0 0 0.75rem;
    font-size: 1.25rem;
  }
  .body {
    color: var(--ink-muted, #a8a8a8);
  }
  form {
    display: flex;
    justify-content: flex-end;
    margin-top: 1.5rem;
  }
  @keyframes rise {
    from {
      opacity: 0;
      transform: translateY(0.5rem);
    }
  }
  @media (prefers-reduced-motion: reduce) {
    dialog[open] {
      animation: none;
    }
  }
</style>
