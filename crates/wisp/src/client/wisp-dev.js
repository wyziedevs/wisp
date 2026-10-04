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
    <style>:host { all: initial; } .wisp-code pre a { color: var(--wisp-accent); text-decoration: none; } .wisp-code pre a:hover { text-decoration: underline; }</style>
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
    const [title, summary, label] = meta ?? ['Build Failed', '', 'Compiler Output'];
    meta = null;
    root.getElementById('title').textContent = title;
    root.getElementById('summary').textContent = summary;
    root.getElementById('summary').hidden = !summary;
    root.getElementById('label').textContent = label;
    // `src/routes/+page.rs:4:5` opens in the editor, as the devtools' "Open" does.
    pre.replaceChildren();
    let at = 0;
    for (const m of text.matchAll(/\b((?:src|static|tests?|examples)\/[^\s:'"`<>]+\.\w+):(\d+)/g)) {
      pre.append(text.slice(at, m.index));
      const a = document.createElement('a');
      a.href = '#';
      a.textContent = m[0];
      a.title = 'Open in the editor';
      a.onclick = (e) => {
        e.preventDefault();
        fetch('/_wisp/dev/open', { method: 'POST', headers: { 'x-wisp-dev': '1' }, body: `${m[1]}\n${m[2]}` });
      };
      pre.append(a);
      at = m.index + m[0].length;
    }
    pre.append(text.slice(at));
    if (!dialog.open) {
      dialog.showModal();
      if (matchMedia('(pointer: fine)').matches) close.classList.add('wisp-ring');
    }
  }

  // ---- runtime errors ---------------------------------------------------------

  // What the page's code throws, and what onError hears: the same dialog,
  // closed by the next successful build. A build error on show is kept.
  let broken = false;
  function runtime(e) {
    if (dialog.open && !broken) return;
    const err = e instanceof Error ? e : new Error(String(e));
    meta = ['Runtime Error', err.message.split('\n')[0], 'Browser Console'];
    showError(err.stack || err.message);
    broken = true;
  }
  addEventListener('error', (e) => runtime(e.error ?? e.message));
  addEventListener('unhandledrejection', (e) => runtime(e.reason));
  document.addEventListener('wisp:error', (e) => runtime(e.detail?.error));

  // A 5xx of the server (a handler's panic with its `file:line`, say): the
  // dev error page carries the message, and it opens in the same dialog.
  const served = document.getElementById('wisp-server-error');
  if (served) {
    const text = served.content.textContent;
    meta = ['Server Error', text.split('\n')[0], 'Server'];
    showError(text);
    broken = true;
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

  // `hot`: `m URL` per browser module swapped into the app, which a page
  // with browser code loads (live.js swaps one it has in place), then
  // `r file` per template whose text changed, whose part of the page is
  // morphed in (wisp.js); the shell's is the whole page.
  async function hot(lines) {
    const urls = lines.filter((l) => l.startsWith('m ')).map((l) => l.slice(2));
    const files = lines.filter((l) => l.startsWith('r ')).map((l) => l.slice(2));
    try {
      if (globalThis.__wisp_dev) await Promise.all(urls.map((u) => import(u)));
    } catch (e) {
      console.error(e);
      return location.reload();
    }
    if (files.includes('src/app.html')) refresh();
    else if (files.some((f) => marked(f))) document.dispatchEvent(new CustomEvent('wisp:region', { detail: { files } }));
  }

  // Whether the page shows what the template `file` renders.
  function marked(file) {
    const w = document.createTreeWalker(document.body, NodeFilter.SHOW_COMMENT);
    while (w.nextNode()) if (w.currentNode.data === 'w:' + file) return true;
    return false;
  }

  // The page again from the server, morphed in. If that fails (a network
  // error, the morph itself), the page is loaded whole: it must show the
  // new build, whatever happened.
  function refresh() {
    let failed = false;
    const fail = () => (failed = true);
    addEventListener('unhandledrejection', fail);
    const done = () =>
      setTimeout(() => {
        removeEventListener('unhandledrejection', fail);
        if (failed) location.reload();
      }, 50);
    document.dispatchEvent(new CustomEvent('wisp:refresh', { detail: { done } }));
  }

  // How many updates `wisp dev` had sent when this page connected. After a
  // dropped connection (a tab asleep through a rebuild), a different count
  // means a rebuild was missed: load the page whole.
  let updates = null;

  let events;
  function connect() {
    events = new EventSource(`http://127.0.0.1:${port}/events`);
    events.onmessage = message;
    // The browser retries a dropped stream itself, but gives up for good on a
    // refusal (the origin not allowed yet): try again.
    events.onerror = () => {
      if (events.readyState === EventSource.CLOSED) setTimeout(connect, 1000);
    };
  }
  connect();

  function message(e) {
    const nl = e.data.indexOf('\n');
    const kind = nl < 0 ? e.data : e.data.slice(0, nl);
    const data = nl < 0 ? '' : e.data.slice(nl + 1);
    if (kind === 'hello') {
      if (updates !== null && updates !== data) location.reload();
      updates = data;
      return;
    }
    if (kind === 'building') return waiting(true);
    if (kind === 'title') return void (meta = data.split('\n'));
    waiting(false);
    if (kind === 'reload' || kind === 'hot' || kind === 'full' || kind === 'css') updates = String(Number(updates) + 1);
    // Said with a rebuild's `reload`: why its modules swap whole.
    if (globalThis.__wisp_dev) __wisp_dev.full = kind === 'reload' ? data : '';
    broken = false;
    if (kind === 'error') return showError(data);
    dialog.close();
    if (kind === 'reload') refresh();
    else if (kind === 'hot') hot(data.split('\n'));
    else if (kind === 'full') location.reload();
    else if (kind === 'css') swapStylesheet();
  }
})();
