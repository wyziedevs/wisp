// wisp-dev.js: debug builds only. Listens to `wisp dev` and applies what it
// says: morph in a rebuilt page, swap the stylesheet, show a build error, or
// draw the waiting line while a rebuild runs.
//
// The dialog lives in a shadow root, so the app's CSS cannot restyle it, and
// hangs off <html> rather than <body>, so a page morph leaves it be. Its
// styles are Wisp's own (ui.css, dialog.css), fetched as the page loads, so
// they are there even when the app is down by the time an error arrives.
(() => {
  const port = document.currentScript.dataset.port;

  // ---- the dialog -------------------------------------------------------------

  // The failure glyph, shared with the error page.
  const FAILED = `<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5" aria-hidden="true">
    <circle cx="8" cy="8" r="6.25"/><path d="M8 4.75v3.75" stroke-linecap="round"/>
    <circle cx="8" cy="11" r=".75" fill="currentColor" stroke="none"/></svg>`;
  const CLOSE = `<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" aria-hidden="true">
    <path d="M4 4l8 8M12 4l-8 8"/></svg>`;

  const host = document.createElement('wisp-dev');
  const root = host.attachShadow({ mode: 'open' });
  root.innerHTML = `
    <style>:host { all: initial; }</style>
    <link rel="stylesheet" href="/_app/wisp-ui.css">
    <link rel="stylesheet" href="/_app/wisp-dialog.css">
    <div class="wisp-wait" hidden></div>
    <dialog class="wisp-dialog" aria-labelledby="title" aria-describedby="summary">
      <div class="wisp-wait" hidden></div>
      <header class="wisp-dialog-head">
        ${FAILED}
        <div class="wisp-dialog-title">
          <h2 id="title"></h2>
          <p id="summary"></p>
        </div>
        <button class="wisp-button wisp-ghost wisp-icon" aria-label="Close" autofocus>${CLOSE}</button>
      </header>
      <section class="wisp-code">
        <div class="wisp-code-strip">
          <span id="label"></span>
          <button class="wisp-button wisp-ghost wisp-small" id="copy">Copy</button>
        </div>
        <pre></pre>
      </section>
    </dialog>`;
  document.documentElement.append(host);

  const dialog = root.querySelector('dialog');
  const [pageWait, dialogWait] = root.querySelectorAll('.wisp-wait');
  const close = root.querySelector('.wisp-icon');
  const copy = root.getElementById('copy');
  const pre = root.querySelector('pre');

  close.onclick = () => dialog.close();
  // On a desk the way out wears the focus ring from the moment the dialog
  // opens, so it is the first thing seen; a press or a blur takes it off.
  const unring = () => close.classList.remove('wisp-ring');
  close.addEventListener('blur', unring);
  close.addEventListener('pointerdown', unring);

  copy.onclick = async () => {
    try {
      await navigator.clipboard.writeText(pre.textContent);
      copy.textContent = 'Copied';
    } catch {
      // No clipboard here (a page not on localhost, say): select it instead.
      getSelection().selectAllChildren(pre);
      copy.textContent = 'Selected';
    }
    setTimeout(() => (copy.textContent = 'Copy'), 1500);
  };

  // Title, summary and the code strip's label, sent by `wisp dev` just before
  // the error. An older `wisp dev` sends none.
  let meta = null;

  function showError(text) {
    const [title, summary, label] = meta ?? ['Build Failed', 'Save a fix to rebuild.', 'Compiler Output'];
    meta = null;
    root.getElementById('title').textContent = title;
    root.getElementById('summary').textContent = summary;
    root.getElementById('label').textContent = label;
    pre.textContent = text;
    if (!dialog.open) {
      dialog.showModal();
      if (matchMedia('(pointer: fine)').matches) close.classList.add('wisp-ring');
    }
  }

  // ---- the waiting line -------------------------------------------------------

  // Shown once a rebuild has taken 200ms, so a quick one draws nothing, and
  // gone the moment anything arrives.
  let waitTimer = 0;
  function waiting(on) {
    clearTimeout(waitTimer);
    pageWait.hidden = dialogWait.hidden = true;
    if (on) waitTimer = setTimeout(() => ((dialog.open ? dialogWait : pageWait).hidden = false), 200);
  }

  // ---- events from `wisp dev` -------------------------------------------------

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
    const data = nl < 0 ? '' : e.data.slice(nl + 1);
    if (kind === 'building') return waiting(true);
    if (kind === 'title') return void (meta = data.split('\n'));
    waiting(false);
    if (kind === 'error') return showError(data);
    dialog.close();
    if (kind === 'reload') document.dispatchEvent(new Event('wisp:refresh'));
    else if (kind === 'full') location.reload();
    else if (kind === 'css') swapStylesheet();
  };
})();
