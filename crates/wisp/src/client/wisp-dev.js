// wisp-dev.js: debug builds only. Listens to `wisp dev` and applies what it
// says: morph in a rebuilt page, swap the stylesheet, or show a build error.
//
// The error dialog lives in a shadow root, so the app's CSS cannot restyle
// it, and hangs off <html> rather than <body>, so a page morph leaves it be.
(() => {
  const port = document.currentScript.dataset.port;

  // ---- error dialog ---------------------------------------------------------

  const STYLE = `
    :host {
      --panel: #1f1f1f;
      --panel-hover: #2e2e2e;
      --inset: #1a1a1a;
      --inset-hover: #2e2e2e;
      --line: #4b4b4b;
      --ink: #fafafa;
      --ink-muted: #a8a8a8;
      --ink-subtle: #949494;
      --accent: #896ce0;
      --danger: #ef8780;
      --enter: cubic-bezier(0.05, 0.7, 0.1, 1);
      --exit: cubic-bezier(0.33, 1, 0.68, 1);
      --standard: cubic-bezier(0.25, 1, 0.5, 1);
      all: initial;
      color-scheme: dark;
    }

    /* Arrives by fading in as it rises 16px into place; leaves by fading
       where it is. The rise is layout (top), never a transform, so the text
       stays crisp while it moves. */
    dialog {
      box-sizing: border-box;
      position: fixed;
      top: 4rem;
      width: min(56rem, 100vw - 2rem);
      max-width: none;
      max-height: calc(100vh - 8rem);
      margin: 0 auto;
      padding: 0;
      overflow: hidden;
      border: 1px solid var(--line);
      border-radius: 0.5rem;
      background: var(--panel);
      box-shadow: 0 16px 40px rgb(0 0 0 / 0.3), 0 4px 12px rgb(0 0 0 / 0.23);
      color: var(--ink);
      font: 400 0.875rem/1.25rem "Open Sans Variable", "Open Sans", "Segoe UI Variable", "Segoe UI",
        -apple-system, BlinkMacSystemFont, system-ui, sans-serif;
      opacity: 0;
      transition: opacity 200ms var(--exit), display 200ms allow-discrete, overlay 200ms allow-discrete;
    }

    dialog[open] {
      display: flex;
      flex-direction: column;
      opacity: 1;
      transition: opacity 450ms var(--enter), top 450ms var(--enter), display 450ms allow-discrete,
        overlay 450ms allow-discrete;
    }

    @starting-style {
      dialog[open] {
        top: calc(4rem + 16px);
        opacity: 0;
      }
    }

    dialog::backdrop {
      background: rgb(0 0 0 / 0);
      transition: background-color 200ms cubic-bezier(0.33, 1, 0.68, 1), display 200ms allow-discrete,
        overlay 200ms allow-discrete;
    }

    dialog[open]::backdrop {
      background: rgb(0 0 0 / 0.5);
      transition-duration: 450ms;
      transition-timing-function: cubic-bezier(0.05, 0.7, 0.1, 1);
    }

    @starting-style {
      dialog[open]::backdrop {
        background: rgb(0 0 0 / 0);
      }
    }

    header {
      display: flex;
      align-items: center;
      gap: 0.75rem;
      padding: 0.75rem 0.75rem 0.75rem 1rem;
      border-bottom: 1px solid var(--line);
    }

    .icon {
      flex: none;
      width: 1rem;
      height: 1rem;
      color: var(--danger);
    }

    .heading {
      flex: 1;
      min-width: 0;
    }

    h2 {
      margin: 0;
      font-size: 0.875rem;
      font-weight: 600;
      line-height: 1.25rem;
    }

    header p {
      margin: 0.125rem 0 0;
      color: var(--ink-muted);
      font-size: 0.75rem;
      line-height: 1rem;
    }

    button {
      display: inline-grid;
      place-items: center;
      padding: 0;
      border: 1px solid transparent;
      border-radius: 0.375rem;
      background: none;
      color: var(--ink-muted);
      font: inherit;
      cursor: pointer;
      transition: background-color 250ms var(--standard), border-color 250ms var(--standard),
        color 250ms var(--standard);
    }

    button:hover {
      border-color: var(--line);
      background: var(--panel-hover);
      color: var(--ink);
    }

    button:active {
      background-image: linear-gradient(rgb(250 250 250 / 0.08) 0 0);
    }

    button:focus-visible {
      outline: 2px solid var(--accent);
      outline-offset: 2px;
    }

    .close {
      flex: none;
      width: 2.25rem;
      height: 2.25rem;
    }

    .close svg {
      width: 1rem;
      height: 1rem;
    }

    .code {
      display: flex;
      flex-direction: column;
      min-height: 0;
      margin: 1rem;
      overflow: hidden;
      border: 1px solid var(--line);
      border-radius: 0.5rem;
      background: var(--inset);
    }

    .strip {
      display: flex;
      align-items: center;
      justify-content: space-between;
      padding: 0.25rem 0.25rem 0.25rem 0.75rem;
      border-bottom: 1px solid var(--line);
      color: var(--ink-subtle);
      font-size: 0.6875rem;
      font-weight: 600;
      letter-spacing: 0.025em;
      line-height: 1rem;
      text-transform: uppercase;
    }

    .copy {
      height: 2rem;
      padding: 0 0.625rem;
      font-size: 0.75rem;
      font-weight: 500;
      letter-spacing: normal;
      text-transform: none;
    }

    .copy:hover {
      background: var(--inset-hover);
    }

    pre {
      flex: 1;
      margin: 0;
      padding: 0.75rem;
      overflow: auto;
      color: var(--ink);
      font: 400 0.75rem/1.25rem "Cascadia Code", "JetBrains Mono", ui-monospace, SFMono-Regular, Menlo,
        monospace;
      tab-size: 4;
    }

    @media (prefers-reduced-motion: reduce) {
      dialog,
      dialog::backdrop {
        transition-duration: 1ms !important;
      }
    }
  `;

  const MARKUP = `
    <style>${STYLE}</style>
    <dialog aria-labelledby="title" aria-describedby="summary">
      <header>
        <svg class="icon" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5" aria-hidden="true">
          <circle cx="8" cy="8" r="6.25" />
          <path d="M8 4.75v3.75" stroke-linecap="round" />
          <circle cx="8" cy="11" r="0.75" fill="currentColor" stroke="none" />
        </svg>
        <div class="heading">
          <h2 id="title">Build Failed</h2>
        </div>
        <button class="close" aria-label="Close" autofocus>
          <svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" aria-hidden="true">
            <path d="M4 4l8 8M12 4l-8 8" />
          </svg>
        </button>
      </header>
      <section class="code">
        <div class="strip">
          <span>Compiler Output</span>
          <button class="copy">Copy</button>
        </div>
        <pre></pre>
      </section>
    </dialog>
  `;

  let dialog = null;

  function showError(message) {
    if (!dialog) {
      const host = document.createElement('wisp-error');
      const root = host.attachShadow({ mode: 'open' });
      root.innerHTML = MARKUP;
      document.documentElement.append(host);
      dialog = root.querySelector('dialog');
      root.querySelector('.close').onclick = () => dialog.close();
      const copy = root.querySelector('.copy');
      copy.onclick = async () => {
        await navigator.clipboard.writeText(dialog.querySelector('pre').textContent);
        copy.textContent = 'Copied';
        setTimeout(() => (copy.textContent = 'Copy'), 1500);
      };
    }
    dialog.querySelector('pre').textContent = message;
    if (!dialog.open) dialog.showModal();
  }

  // ---- events from `wisp dev` -----------------------------------------------

  function swapStylesheet() {
    for (const link of document.querySelectorAll('link[rel=stylesheet][href^="/_app/app.css"]')) {
      const fresh = link.cloneNode();
      fresh.href = '/_app/app.css?v=' + Date.now();
      fresh.onload = () => link.remove(); // swap after load: no unstyled flash
      link.after(fresh);
    }
  }

  const events = new EventSource(`http://127.0.0.1:${port}/events`);
  events.onmessage = (e) => {
    const nl = e.data.indexOf('\n');
    const kind = nl < 0 ? e.data : e.data.slice(0, nl);
    if (kind === 'error') return showError(e.data.slice(nl + 1));
    dialog?.close();
    if (kind === 'reload') document.dispatchEvent(new Event('wisp:refresh'));
    else if (kind === 'full') location.reload();
    else if (kind === 'css') swapStylesheet();
  };
})();
