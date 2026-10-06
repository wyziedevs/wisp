// wisp-devtools.js: debug builds only. Alt+Shift+W opens a panel over the
// page: the component tree with each instance's live state (editable) and
// props, the stores, the route with its params and server data, the last
// navigation's or action's timings, and a way to open each file in the
// editor (POST /_wisp/dev/open).
//
// It reads what live.js keeps for it in debug builds, globalThis.__wisp_dev:
// the instances, their state's signals, the stores. An edit writes a signal,
// so the page redraws as it would for its own code. A page with no browser
// code has no live.js, and shows its route alone. Like the build error
// dialog, the panel lives in a shadow root hung off <html>, which morphs
// leave be and the app's CSS cannot reach.
(() => {
  let root = null; // the panel's shadow root, made on first open
  let shown = false;
  let tab = 'Components';
  let picked = null; // the instance whose details show
  let timer = 0;
  let pressed = false; // a pointer is down in the panel
  let last = null; // the last navigation's or action's timings
  let pending = null;

  addEventListener(
    'keydown',
    (e) => {
      if (e.altKey && e.shiftKey && !e.ctrlKey && !e.metaKey && e.code === 'KeyW') {
        e.preventDefault();
        toggle();
      }
    },
    true,
  );

  // ---- timings ----------------------------------------------------------------

  const now = () => performance.now();
  const on = (type, f) => document.addEventListener(type, f, true); // capture: a form's events too
  const where = (u) => {
    const url = new URL(String(u ?? location.href), location.href);
    return url.pathname + url.search;
  };
  on('wisp:navigate', (e) => (pending = { kind: 'Navigation', what: where(e.detail?.to), t: now() }));
  on('wisp:submit', (e) => (pending = { kind: 'Action', what: where(e.detail?.action), t: now() }));
  on('wisp:update', (e) => pending?.kind === 'Navigation' && finish(e.detail?.status));
  on('wisp:result', (e) => pending?.kind === 'Action' && finish(e.detail?.status));

  // The request's share comes from its resource timing: the wait for the
  // first byte (the server's work, mostly), the download, then the rest,
  // which is the morph and the redraw.
  function finish(status) {
    const p = pending;
    const end = now();
    pending = null;
    const r = performance.getEntriesByType('resource').filter((x) => x.initiatorType === 'fetch' && x.startTime >= p.t).at(-1);
    last = { ...p, status, total: end - p.t, wait: r && r.responseStart - r.requestStart, load: r && r.responseEnd - r.responseStart, draw: r && end - r.responseEnd };
    if (shown) draw();
  }

  function firstLoad() {
    const n = performance.getEntriesByType('navigation')[0];
    if (!n) return null;
    const end = n.domContentLoadedEventEnd || now();
    return { kind: 'Page load', what: where(), status: n.responseStatus, total: end - n.startTime, wait: n.responseStart - n.requestStart, load: n.responseEnd - n.responseStart, draw: end - n.responseEnd };
  }

  // ---- the panel --------------------------------------------------------------

  function toggle() {
    shown = !shown;
    if (shown && !root) build();
    root.querySelector('.panel').hidden = !shown;
    clearInterval(timer);
    if (!shown) return;
    draw();
    // Live values: drawn again while open, but not under a field being typed in.
    // A press is left to end first: a click needs the element it began on.
    timer = setInterval(() => pressed || root.activeElement?.localName === 'input' || draw(), 500);
  }

  function build() {
    const host = document.createElement('wisp-devtools');
    root = host.attachShadow({ mode: 'open' });
    root.innerHTML = `<style>:host { all: initial; }</style>
      <link rel="stylesheet" href="/_app/wisp-ui.css">
      <style>${CSS}</style>
      <aside class="panel" role="dialog" aria-label="Wisp devtools">
        <header>
          <b>Wisp</b>
          <nav role="tablist"></nav>
          <button class="wisp-button wisp-ghost wisp-icon wisp-small" aria-label="Close" title="Close (Alt+Shift+W)">${CLOSE}</button>
        </header>
        <div class="body"></div>
      </aside>`;
    root.querySelector('header button').onclick = toggle;
    // Keys typed in the panel are the panel's: the page's own shortcuts
    // (on:keydown.window and the like) never see them.
    const panel = root.querySelector('.panel');
    for (const t of ['keydown', 'keypress', 'keyup']) panel.addEventListener(t, (e) => e.stopPropagation());
    panel.addEventListener('pointerdown', () => (pressed = true));
    addEventListener('pointerup', () => (pressed = false), true);
    root.querySelector('nav').append(
      ...['Components', 'Stores', 'Route', 'Routes', 'Timings'].map((t) => h('button', { role: 'tab', textContent: t, onclick: () => ((tab = t), draw()) })),
    );
    // Shown once Wisp's styles are in, so it never flashes unstyled.
    panel.style.visibility = 'hidden';
    root.querySelector('link').onload = root.querySelector('link').onerror = () => (panel.style.visibility = '');
    document.documentElement.append(host);
  }

  const CLOSE = `<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" aria-hidden="true"><path d="M4 4l8 8M12 4l-8 8"/></svg>`;

  // An element: h('p', { className: 'x' }, 'text', child).
  function h(tag, props, ...kids) {
    const el = Object.assign(document.createElement(tag), props);
    el.append(...kids.flat().filter((k) => k != null && k !== false));
    return el;
  }

  function draw() {
    for (const b of root.querySelectorAll('[role=tab]')) b.ariaSelected = String(b.textContent === tab);
    const dev = globalThis.__wisp_dev;
    const body = root.querySelector('.body');
    const top = body.scrollTop;
    const parts = tab === 'Components' ? components(dev) : tab === 'Stores' ? stores(dev) : tab === 'Route' ? route(dev) : tab === 'Routes' ? routes() : timings();
    body.replaceChildren(...parts.flat().filter((p) => p != null && p !== false));
    body.scrollTop = top;
  }

  const none = (text) => h('p', { className: 'none', textContent: text });
  const NO_CODE = 'This page has no browser code, so nothing here runs in the browser.';

  // ---- components ---------------------------------------------------------------

  // `src/components/Card.wisp` is Card; a route's file is its path.
  const nameOf = (file) =>
    file.startsWith('src/components/') ? file.slice(15).replace(/\.wisp$/, '') : file.replace(/^src\/routes\/?/, '').replace(/\.wisp$/, '') || file;

  function components(dev) {
    if (!dev) return [none(NO_CODE)];
    const all = dev.live();
    if (!all.length) return [none('No instances running.')];
    if (!all.includes(picked)) picked = all[0];
    const kids = new Map();
    for (const i of all) {
      const up = all.includes(i.parent) ? i.parent : null;
      kids.set(up, [...(kids.get(up) || []), i]);
    }
    const tree = (up) =>
      kids.has(up) &&
      h(
        'ul',
        {},
        kids.get(up).map((i) =>
          h(
            'li',
            {},
            h('button', { className: 'node', ariaCurrent: String(i === picked), textContent: nameOf(i.file), onclick: () => ((picked = i), draw()) }),
            tree(i),
          ),
        ),
      );
    return [h('div', { className: 'tree' }, tree(null)), details(picked)];
  }

  function details(i) {
    const props = Object.entries(i.P || {});
    const state = Object.entries(i.st || {});
    return h(
      'section',
      { className: 'details' },
      h('div', { className: 'title' }, h('h2', { textContent: nameOf(i.file) }), opener(i.file, 1)),
      h('h3', { textContent: 'Props' }),
      props.length ? props.map(([k, s]) => row(k === '__rest' ? '...rest' : k, s)) : none('None it reads in the browser.'),
      h('h3', { textContent: 'State' }),
      state.length ? state.map(([k, s]) => row(k, s, opener(i.file, i.lines?.[k] || 1, 'line ' + (i.lines?.[k] || 1)))) : none('No state.'),
      h('h3', { textContent: 'Graph' }),
      state.length ? h('ul', { className: 'graph' }, graph(state)) : none('No state.'),
    );
  }

  // What reads each signal now: the state and $derived values it feeds, by
  // name, and how many effects and DOM updates. A signal keeps its readers
  // as [node, slot, ...] in `subs`; a $derived is a node with `memo`, an
  // $effect one with `u`.
  function graph(state) {
    const names = new Map(state.map(([k, s]) => [s, k]));
    return state.map(([k, s]) => {
      const by = {};
      for (let j = 0; s.subs && j < s.subs.length; j += 2) {
        const n = s.subs[j];
        const what = names.get(n) || (n.memo ? 'derived' : n.u ? 'effect' : 'DOM');
        by[what] = (by[what] || 0) + 1;
      }
      const reads = Object.entries(by).map(([w, c]) => (['derived', 'effect', 'DOM'].includes(w) ? `${c} ${w}` : w));
      return h('li', {}, h('code', { textContent: k }), ' → ', reads.length ? reads.join(', ') : 'nothing reads it');
    });
  }

  // ---- values -------------------------------------------------------------------

  // A signal's value, with the field that edits it: a checkbox, a number, a
  // text, or JSON for the rest. A `$derived` value only shows.
  function row(label, sig, extra, readonly) {
    const v = sig.v;
    const fixed = !!sig.memo || !!readonly;
    let field;
    if (typeof v === 'boolean') {
      field = h('input', { type: 'checkbox', checked: v, disabled: fixed, onchange: (e) => (sig.v = e.target.checked) });
    } else if (typeof v === 'number') {
      field = h('input', { type: 'number', step: 'any', value: v, disabled: fixed, oninput: (e) => e.target.value !== '' && (sig.v = +e.target.value) });
    } else if (typeof v === 'string') {
      field = h('input', { value: v, disabled: fixed, oninput: (e) => (sig.v = e.target.value) });
    } else if (v === undefined || v === null || Array.isArray(v) || Object.getPrototypeOf(v) === Object.prototype) {
      field = h('input', { className: 'json', value: json(v), disabled: fixed, spellcheck: false, onchange: (e) => edit(sig, e.target) });
    } else {
      field = h('code', { textContent: typeof v === 'function' ? `ƒ ${v.name || ''}()` : String(v) });
    }
    return h('div', { className: 'row' }, h('span', { className: 'key', textContent: label }, sig.memo && h('small', { textContent: ' derived' }), extra), field);
  }

  function json(v) {
    if (v === undefined) return 'undefined';
    try {
      return JSON.stringify(v);
    } catch {
      return String(v);
    }
  }

  function edit(sig, input) {
    try {
      sig.v = input.value.trim() === 'undefined' ? undefined : JSON.parse(input.value);
      input.classList.remove('bad');
    } catch {
      input.classList.add('bad');
    }
  }

  const opener = (file, line, text = 'Open') =>
    h('button', {
      className: 'link',
      textContent: text,
      title: `Open ${file}:${line} in the editor`,
      onclick: () => fetch('/_wisp/dev/open', { method: 'POST', headers: { 'x-wisp-dev': '1' }, body: `${file}\n${line}` }),
    });

  // ---- stores -------------------------------------------------------------------

  // Where a store was made: the first frame of its stack outside the
  // runtime. A module's frames name its .wisp file (its sourceURL); a lib
  // file's are its URL, /_app/c/lib/x.js, which is src/lib/x.js.
  function madeAt(stack) {
    for (const line of String(stack).split('\n').slice(1)) {
      const m = /(wisp:\/\/\/[^\s)]+|https?:\/\/[^\s)]+):(\d+):\d+\)?\s*$/.exec(line);
      if (!m || /\/_app\/(live|c\/extra)\.js/.test(m[1])) continue;
      const url = m[1];
      const file = url.startsWith('wisp:///') ? url.slice(8) : 'src/lib/' + new URL(url).pathname.replace(/^\/_app\/c\/lib\//, '');
      return { url, file, line: +m[2] };
    }
    return null;
  }

  // A store's name, read from the line that made it: `export const cart =
  // store([])` is cart. Fetched once per file.
  const sources = new Map();
  function nameAt(at) {
    if (!at.url.startsWith('http')) return null;
    if (!sources.has(at.url)) {
      sources.set(at.url, null);
      fetch(at.url)
        .then((r) => r.text())
        .then((t) => (sources.set(at.url, t.split('\n')), shown && draw()), () => {});
    }
    const line = sources.get(at.url)?.[at.line - 1];
    return line && /(?:const|let|var)\s+([\w$]+)\s*=/.exec(line)?.[1];
  }

  function stores(dev) {
    if (!dev) return [none(NO_CODE)];
    // live.js makes `page` and `navigating` first, before any of the app's.
    return dev.stores.map(([sig, stack], k) => {
      const at = madeAt(stack);
      if (!at) return row(['page', 'navigating'][k] || 'store', sig, null, true); // the runtime's: they only show
      const name = nameAt(at) || `${at.file.split('/').pop()}:${at.line}`;
      return row(name, sig, opener(at.file, at.line, at.file.split('/').pop() + ':' + at.line));
    });
  }

  // ---- route --------------------------------------------------------------------

  function route(dev) {
    const out = [h('div', { className: 'row' }, h('span', { className: 'key', textContent: 'URL' }), h('code', { textContent: where() }))];
    if (!dev) return [...out, none(NO_CODE)];
    const r = dev.route();
    if (r.id) out.push(h('div', { className: 'row' }, h('span', { className: 'key', textContent: 'Route' }), h('code', { textContent: r.id })));
    const params = Object.entries(r.params || {});
    out.push(h('h3', { textContent: 'Params' }));
    out.push(params.length ? params.map(([k, v]) => h('div', { className: 'row' }, h('span', { className: 'key', textContent: k }), h('code', { textContent: v }))) : none(r.id ? 'None.' : 'Sent to a page with a +page.js load.'));
    // The server values each page and layout reads in the browser.
    for (const i of dev.live().filter((i) => /\+(page|layout)\.wisp$/.test(i.file))) {
      out.push(h('h3', { textContent: 'data · ' + nameOf(i.file) }));
      const vals = Object.entries(i.P || {});
      out.push(vals.length ? vals.map(([k, s]) => row(k, s)) : none('It reads no server value in the browser.'));
    }
    // The page's forms: where each posts, and the fields it sends.
    out.push(h('h3', { textContent: 'Forms' }));
    const forms = [...document.forms];
    out.push(
      forms.length
        ? forms.map((f) => {
            const names = [...f.elements].map((e) => e.name).filter(Boolean);
            return h('div', { className: 'row' }, h('span', { className: 'key', textContent: f.method.toUpperCase() + ' ' + (f.getAttribute('action') || where()) }), h('code', { textContent: names.join(', ') || 'no fields' }));
          })
        : none('None on this page.'),
    );
    return out;
  }

  // ---- routes -------------------------------------------------------------------

  let table = null; // the app's routes, fetched once: [pattern, has a page]

  function routes() {
    if (!table) {
      table = [];
      fetch('/_wisp/dev/routes')
        .then((r) => r.json())
        .then((t) => ((table = t), draw()))
        .catch(() => (table = null));
      return [none('Loading…')];
    }
    const here = globalThis.__wisp_dev?.route().id;
    return table.map(([pattern, page]) =>
      h('div', { className: 'row' }, h('span', { className: 'key', textContent: pattern, title: page ? 'page' : 'endpoint' }), h('code', { textContent: (page ? 'page' : 'endpoint') + (pattern === here ? ' (here)' : '') })),
    );
  }

  // ---- timings ------------------------------------------------------------------

  function timings() {
    const t = last || firstLoad();
    if (!t) return [none('Nothing timed yet: navigate, or submit a form.')];
    const ms = (x) => (x == null || !(x >= 0) ? '—' : x < 10 ? x.toFixed(1) + ' ms' : Math.round(x) + ' ms');
    const line = (k, v) => h('div', { className: 'row' }, h('span', { className: 'key', textContent: k }), h('code', { textContent: v }));
    return [
      h('h3', { textContent: t.kind }),
      line('Where', t.what),
      t.status && line('Status', String(t.status)),
      line('Total', ms(t.total)),
      line('Server wait', ms(t.wait)),
      line('Download', ms(t.load)),
      line(t.kind === 'Page load' ? 'Parse and start' : 'Morph and redraw', ms(t.draw)),
      pending && none(`${pending.kind} to ${pending.what} in progress…`),
    ];
  }

  const CSS = `
    .panel[hidden] { display: none; }
    .panel { position: fixed; z-index: 2147483646; top: 0; right: 0; bottom: 0; display: flex; flex-direction: column; box-sizing: border-box; width: min(26rem, 100vw);
      border-left: 1px solid var(--wisp-line); background: var(--wisp-panel); box-shadow: var(--wisp-overlay); color: var(--wisp-ink); font: 400 0.8125rem/1.25rem var(--wisp-sans); }
    header { display: flex; align-items: center; gap: 0.75rem; padding: 0.5rem 0.5rem 0.5rem 1rem; border-bottom: 1px solid var(--wisp-line); }
    header b { font-weight: 600; letter-spacing: -0.01em; }
    nav { display: flex; flex: 1; gap: 0.125rem; }
    [role=tab] { padding: 0.25rem 0.5rem; border: 0; border-radius: var(--wisp-radius); background: none; color: var(--wisp-slate); font: 500 0.75rem/1rem var(--wisp-sans); cursor: pointer; }
    [role=tab]:hover { background: var(--wisp-inset); color: var(--wisp-ink); }
    [role=tab][aria-selected=true] { background: var(--wisp-inset); color: var(--wisp-ink); box-shadow: inset 0 -2px var(--wisp-accent); }
    .body { flex: 1; overflow: auto; padding: 0.75rem 1rem 1rem; }
    .tree ul { margin: 0; padding: 0 0 0 0.75rem; list-style: none; }
    .tree > ul { padding: 0; }
    .node { display: block; width: 100%; padding: 0.25rem 0.5rem; border: 0; border-radius: var(--wisp-radius); background: none; color: var(--wisp-ink); font: 500 0.8125rem/1.25rem var(--wisp-mono); text-align: left; cursor: pointer; }
    .node:hover { background: var(--wisp-inset); }
    .node[aria-current=true] { background: var(--wisp-inset); box-shadow: inset 2px 0 var(--wisp-accent); }
    .details { margin-top: 0.75rem; padding-top: 0.75rem; border-top: 1px solid var(--wisp-line); }
    .graph { margin: 0; padding-left: 1rem; font-size: 0.8125rem; }
    .title { display: flex; align-items: center; justify-content: space-between; gap: 0.5rem; }
    h2 { margin: 0; font: 600 0.875rem/1.25rem var(--wisp-mono); }
    h3 { margin: 1rem 0 0.375rem; color: var(--wisp-slate); font-size: 0.6875rem; line-height: 1rem; font-weight: 600; letter-spacing: 0.05em; text-transform: uppercase; }
    .row { display: grid; grid-template-columns: minmax(6rem, 40%) 1fr; align-items: center; gap: 0.5rem; padding: 0.1875rem 0; }
    .key { overflow: hidden; color: var(--wisp-slate); font-family: var(--wisp-mono); text-overflow: ellipsis; white-space: nowrap; }
    .key small { color: var(--wisp-ash); }
    .key .link { margin-left: 0.375rem; }
    code { overflow-wrap: anywhere; font: 400 0.75rem/1rem var(--wisp-mono); }
    input { box-sizing: border-box; width: 100%; min-width: 0; padding: 0.1875rem 0.5rem; border: 1px solid var(--wisp-line); border-radius: var(--wisp-radius); background: var(--wisp-inset); color: var(--wisp-ink); font: 400 0.75rem/1.25rem var(--wisp-mono); }
    input[type=checkbox] { justify-self: start; width: 1rem; height: 1rem; accent-color: var(--wisp-accent); }
    input:disabled { opacity: 0.7; }
    input.bad { border-color: var(--wisp-danger); }
    input:focus-visible, button:focus-visible { outline: 2px solid var(--wisp-accent); outline-offset: 1px; }
    .link { padding: 0; border: 0; background: none; color: var(--wisp-accent); font: 500 0.75rem/1rem var(--wisp-sans); cursor: pointer; }
    .link:hover { text-decoration: underline; }
    .none { margin: 0.25rem 0; color: var(--wisp-ash); }
  `;
})();
