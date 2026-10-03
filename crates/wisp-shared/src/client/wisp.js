// wisp.js: progressive enhancement for Wisp pages. No dependencies.
//
// Links and back/forward: the next page is fetched and morphed into the
// current one, so the layout's DOM (and the state of its browser code) stays,
// with the scroll put back and focus reset. A link is left to the browser
// with data-wisp-reload (on it or around it), a target, download,
// rel="external", or to another site. Hovering or touching a link fetches
// its page ahead; data-wisp-preload="off" stops that.
//
// A <form method="post"> is sent with fetch and the page the server answers
// with is morphed in: nodes are reused where they match, so focus, scroll,
// <details> state and unsaved input elsewhere survive. Without this script
// the same links and forms work as plain navigations and posts.
//
// A link takes data-wisp-noscroll (stay where the page is),
// data-wisp-keepfocus (focus stays) and data-wisp-replacestate (no history
// entry), on it or around it.
//
// Events on the document: `wisp:navigate` before a navigation (cancelable),
// `wisp:leave` before the page changes (`detail.w` collects promises it
// waits for; live.js's onNavigate), `wisp:preload` (`{url, code, done}`),
// `wisp:stale` when a page names a newer wisp.js, `wisp:update`
// after each morph (live.js restarts browser code on it). Dispatching
// `wisp:refresh` morphs in the current URL's page again (`wisp dev` does it
// after every rebuild), `wisp:goto` navigates, `wisp:push` adds a history
// entry with state on the page shown (`wisp:pop` when one comes back). A form gets `wisp:submit`
// (cancelable) before it is sent and `wisp:result` after. An element with
// `data-wisp-keep` is left as it is, for a widget that owns its own DOM.
// Nodes that browser code made (marked __w) are left too.
(() => {
  const headers = { 'x-wisp': '1' };
  const key = (u) => String(u).split('#')[0];
  let shown = key(location.href);
  const me = document.currentScript?.src;
  const ran = new Set([...document.scripts].map((s) => s.src || s.text));
  // Head elements the server sent: a navigation swaps these, and leaves
  // alone what scripts added.
  let served = [...document.head.children];

  // `back`: the history entry whose snapshot goes back in (a pop).
  function swap(html, status = 200, back) {
    const doc = answers(new DOMParser().parseFromString(html, 'text/html'));
    if (doc.title) document.title = doc.title;
    const next = [...doc.head.children];
    const v = doc.querySelector('script[src*="wisp.js"]')?.src;
    if (me && v && v != me) send('wisp:stale');
    served = served.filter((n) => {
      const i = next.findIndex((m) => m.isEqualNode(n));
      if (i < 0) n.remove();
      else next.splice(i, 1);
      return i >= 0;
    });
    document.head.append(...next);
    served.push(...next);
    morph(document.body, doc.body);
    seen = Date.now();
    shown = key(location.href);
    // Parsed scripts are inert; a copy made here runs when inserted.
    for (const old of document.querySelectorAll('script')) {
      const k = old.src || old.text;
      if (ran.has(k) || (old.type && !/module|javascript/.test(old.type))) continue;
      ran.add(k);
      const s = document.createElement('script');
      for (const { name, value } of old.attributes) s.setAttribute(name, value);
      s.text = old.text;
      old.replaceWith(s);
    }
    back ? restore(back) : (pend = null);
    wake(); // before the update: live.js takes the page's JSON over there
    send('wisp:update', { status });
  }

  // `{#await}` answers, streamed after a page: each in its place, its
  // instances added to the page's, as protocol.rs's AWAIT_JS does in a page
  // loaded whole. Its script goes, so it does not run again.
  function answers(doc) {
    for (const d of doc.querySelectorAll('[data-wisp-await]')) {
      const j = d.querySelector('[data-wisp-live]');
      const L = doc.getElementById('wisp-live');
      if (j && L) {
        const [a, b] = [L, j].map((s) => JSON.parse(s.text));
        Object.assign(a.m, b.m);
        a.i.push(...b.i);
        a.t = { ...a.t, ...b.t };
        L.text = JSON.stringify(a);
        j.remove();
      } else if (j) (j.id = 'wisp-live'), doc.body.append(j);
      doc.getElementById('wisp-await-' + d.dataset.wispAwait)?.replaceWith(...d.childNodes);
      d.nextElementSibling?.remove();
      d.remove();
    }
    return doc;
  }

  const send = (type, detail, at = document) => {
    const e = new CustomEvent(type, { detail, cancelable: true });
    at.dispatchEvent(e);
    return !e.defaultPrevented;
  };

  // Where a response sends the page. The server hands its redirects to us
  // as `x-wisp-location`, since fetch would follow them with the post's own
  // headers, and to another site not at all; fetch follows any other.
  // A `javascript:` one is dropped: a browser never follows a redirect
  // there, and `redirect(next)` with a `?next=` from a link must not run it.
  function redirect(res) {
    const to = res.headers.get('x-wisp-location');
    const url = to ? new URL(to, location.href) : res.redirected ? new URL(res.url) : null;
    return url && !script(url) ? url : null;
  }
  const script = (url) => /^(javascript|vbscript):$/.test(url.protocol);

  // A page to morph in: HTML the server does not mean to be saved as a file.
  const isHtml = (res) =>
    (res.headers.get('content-type') || '').startsWith('text/html') && !attachment(res);
  const attachment = (res) => /^\s*attachment/i.test(res.headers.get('content-disposition') || '');

  // A polite live region for what a page load would have said: the new
  // page's title, "offline".
  let live;
  const say = (t) => {
    if (!live?.isConnected) {
      live = document.createElement('div');
      live.setAttribute('aria-live', 'polite');
      live.style.cssText = 'position:absolute;width:1px;height:1px;overflow:hidden;clip-path:inset(50%)';
      live.__w = 1;
      document.body.append(live);
    }
    live.textContent = '';
    setTimeout(() => (live.textContent = t), 50);
  };

  // `<body data-wisp-revalidate>`: back in the tab, or online again, the
  // page's data is fetched anew (at most every 30s, or the attribute's
  // seconds). The morph keeps the focus and the scroll.
  let seen = Date.now();
  const stale = () => {
    const s = document.body.dataset.wispRevalidate;
    if (s == null || document.hidden || !navigator.onLine || Date.now() - seen < (+s || 30) * 1000) return;
    seen = Date.now();
    refresh().catch(() => {});
  };
  document.addEventListener('visibilitychange', stale);
  addEventListener('online', stale);

  async function refresh(extra) {
    const res = await fetch(location.href, { headers: { ...headers, ...extra } });
    const to = redirect(res);
    if (to) return go(to, { replace: true });
    const html = await res.text();
    swap((await drawn(html, location)) || html, res.status);
  }

  // ---- static hosts (`wisp build --spa`) -----------------------------------

  // A static host answers a path it has no file for with index.html, whose
  // #wisp-spa lists the pages the browser draws: [route, file]. The page of
  // the route that fits `url` comes in its place, given the address's
  // parameters. Else null.
  async function drawn(html, url) {
    const list = /<script type="application\/json" id="wisp-spa">([^<]*)<\/script>/.exec(html);
    if (!list) return null;
    for (const [route, file] of JSON.parse(list[1])) {
      const p = fit(route, url.pathname);
      if (!p) continue;
      const res = await fetch(file);
      if (!res.ok) return null;
      const doc = new DOMParser().parseFromString(await res.text(), 'text/html');
      const json = doc.getElementById('wisp-live');
      if (json) json.textContent = JSON.stringify({ ...JSON.parse(json.textContent), r: route, p }).replace(/</g, '\\u003c');
      return '<!doctype html>' + doc.documentElement.outerHTML;
    }
    return null;
  }

  // The parameters of `path` when it fits `route` (`/blog/[slug]`), else null.
  function fit(route, path) {
    let got;
    try {
      got = path.split('/').filter(Boolean).map(decodeURIComponent);
    } catch {
      return null;
    }
    const p = {};
    let i = 0;
    for (const s of route.split('/').filter(Boolean)) {
      const m = /^\[(\[)?(\.\.\.)?([^\]=]+)(?:=(\w+))?\]\]?$/.exec(s);
      if (!m) {
        if (got[i++] !== s) return null;
        continue;
      }
      const [, opt, rest, name, kind] = m;
      if (rest) {
        p[name] = got.slice(i).join('/');
        i = got.length;
        continue;
      }
      const v = got[i];
      if (v === undefined || (kind === 'int' && !/^\d+$/.test(v))) {
        if (opt) continue;
        return null;
      }
      p[name] = v;
      i++;
    }
    return i === got.length ? p : null;
  }

  // ---- navigation -----------------------------------------------------------

  let nav = 0; // the latest navigation; an older one that finishes late is dropped
  const pre = new Map(); // url -> [when, response promise], from hovering
  addEventListener('pagehide', () => {
    save(entry);
    history.scrollRestoration = 'auto';
  });
  addEventListener('pageshow', () => (history.scrollRestoration = 'manual'));

  // A link this script follows.
  function ours(a) {
    if (!(a instanceof HTMLAnchorElement) || !a.href) return false;
    const url = new URL(a.href);
    const t = a.getAttribute('target');
    return (
      url.origin === location.origin &&
      /^https?:$/.test(url.protocol) &&
      !url.pathname.startsWith('/_app/') &&
      !a.hasAttribute('download') &&
      (!t || t === '_self') &&
      !/\bexternal\b/.test(a.rel) &&
      !a.closest('[data-wisp-reload]')
    );
  }

  // The build this page is of (`<meta name="wisp-build">`, release builds).
  const build = document.querySelector('meta[name=wisp-build]')?.content;
  const stale = (html) => build && !html.includes(`name="wisp-build" content="${build}"`);
  async function go(url, how = {}) {
    url = new URL(url, location.href);
    if (script(url)) return; // goto(text from a visitor) runs nothing
    if (url.origin !== location.origin) return location.assign(url);
    // A pop is over: the browser has gone there, so it cannot be canceled.
    if (!send('wisp:navigate', { from: location.href, to: url.href, pop: !!how.pop }) && !how.pop) return;
    const my = ++nav;
    if (!how.pop) history.replaceState({ ...history.state, x: scrollX, y: scrollY }, '');
    let res;
    try {
      const early = pre.get(key(url));
      pre.delete(key(url));
      res = (early && Date.now() - early[0] < 10000 && (await early[1])) || (await fetch(url, { headers }));
      for (let n = 0, to; n < 5 && (to = redirect(res)); n++) {
        if (to.origin !== location.origin) return location.assign(to);
        url = to;
        if (res.redirected) break;
        res = await fetch(to, { headers });
      }
    } catch {
      return location.assign(url);
    }
    if (my !== nav) return;
    if (!isHtml(res)) return location.assign(url); // a file: the browser shows or saves it
    let html = await res.text();
    if (stale(html)) return location.assign(url); // another build: its scripts are not ours
    html = (await drawn(html, url)) || html;
    if (my !== nav) return;
    if (how.replace) history.replaceState({ k: (entry = id()) }, '', url);
    else if (!how.pop) push(url);
    const show = () => {
      swap(html, res.status, how.pop && entry);
      const at = how.pop && history.state;
      if (at) scrollTo(at.x || 0, at.y || 0);
      else if (url.hash) document.getElementById(decodeURIComponent(url.hash.slice(1)))?.scrollIntoView();
      else if (!how.noscroll) scrollTo(0, 0);
      // Focus starts over, as on a page load, unless the page asks for it.
      const auto = document.querySelector('[autofocus]');
      if (auto) auto.focus();
      else if (!how.keepfocus) {
        // To the heading (else the main part), where a screen reader starts
        // reading, and the title said aloud.
        const t = document.querySelector('h1') || document.querySelector('main, [role=main]') || document.body;
        document.activeElement?.blur();
        t.setAttribute('tabindex', '-1');
        t.style.outline = 'none';
        t.focus({ preventScroll: true });
        t.addEventListener('blur', () => (t.removeAttribute('tabindex'), (t.style.outline = '')), { once: true });
        say(document.title);
      }
    };
    // onNavigate's functions run first; what they return runs after.
    const w = [];
    send('wisp:leave', { from: location.href, to: url.href, w });
    const after = await Promise.all(w);
    if (my !== nav) return;
    if (document.startViewTransition && !matchMedia('(prefers-reduced-motion: reduce)').matches) {
      // Its animation is skipped, rejecting these, when the tab is hidden.
      const vt = document.startViewTransition(show);
      vt.ready.catch(() => {});
      vt.finished.catch(() => {});
      await vt.updateCallbackDone;
    } else show();
    for (const f of after) if (typeof f == 'function') f();
  }

  const link = (e) => e.target.closest?.('a[href]');
  document.addEventListener('click', (e) => {
    const a = link(e);
    if (!a || e.defaultPrevented || e.button || e.metaKey || e.ctrlKey || e.shiftKey || e.altKey || !ours(a)) return;
    const url = new URL(a.href);
    // Same page, another #place (or a bare `#`): the browser scrolls there.
    if (a.href.includes('#') && key(url) === key(location.href)) return;
    e.preventDefault();
    const has = (n) => !!a.closest(`[data-wisp-${n}]`);
    go(url, { replace: has('replacestate'), noscroll: has('noscroll'), keepfocus: has('keepfocus') });
  });

  // Fetches a page ahead, used if it is followed within 10s.
  function ahead(href) {
    const k = key(href);
    if (k === key(location.href)) return;
    if (!(pre.has(k) && Date.now() - pre.get(k)[0] < 10000)) pre.set(k, [Date.now(), fetch(k, { headers }).catch(() => null)]);
    return pre.get(k)[1];
  }
  function preload(a) {
    if (a && ours(a) && !a.closest('[data-wisp-preload="off"]')) ahead(a.href);
  }
  // preloadData(url), and preloadCode(url): the modules the page names too.
  document.addEventListener('wisp:preload', async (e) => {
    const { url, code, done } = e.detail;
    const u = new URL(url, location.href);
    const res = u.origin === location.origin && !u.pathname.startsWith('/_app/') && (await ahead(u));
    if (code && res?.ok) {
      const m = /id="wisp-live"[^>]*>([^<]*)/.exec(await res.clone().text());
      for (const href of Object.values((m && JSON.parse(m[1]).m) || {})) {
        const l = document.createElement('link');
        l.rel = 'modulepreload';
        l.href = href;
        document.head.append(l);
      }
    }
    done?.();
  });
  let hover;
  document.addEventListener('mouseover', (e) => {
    clearTimeout(hover);
    const a = link(e);
    if (a) hover = setTimeout(() => preload(a), 60);
  });
  document.addEventListener('touchstart', (e) => preload(link(e)), { passive: true });

  // Back/forward across entries we pushed: show that URL's page. An entry
  // pushState made on the page shown (`p`) needs no request.
  addEventListener('popstate', () => {
    save(entry);
    entry = mark();
    if ((history.state?.p ?? key(location.href)) !== shown) go(location.href, { pop: true });
    else restore(entry), send('wisp:pop');
  });
  // pushState(url, state) and replaceState in a script (live.js).
  document.addEventListener('wisp:push', (e) => {
    const { url, state: s, replace } = e.detail;
    if (replace) history.replaceState({ ...history.state, p: shown, s }, '', url);
    else push(url, { p: shown, s });
  });

  // ---- snapshots ------------------------------------------------------------

  // What a history entry's fields hold where the visitor changed them (not
  // passwords, files, hidden fields or autocomplete="off"), and what scripts'
  // `snapshot.capture()` gave (extra.js), kept in sessionStorage by the
  // entry's key `k`: back, forward and a reload put them back.
  const id = () => Math.random().toString(36).slice(2);
  let entry = history.state?.k;
  let pend = null; // scripts' snapshots for the entry shown (extra.js asks)
  function mark() {
    let k = history.state?.k;
    if (!k) history.replaceState({ ...history.state, k: (k = id()) }, '');
    return k;
  }
  function push(url, state) {
    save(entry);
    history.pushState({ ...state, k: (entry = id()) }, '', url);
  }
  const fields = () => [...document.querySelectorAll('input,textarea,select')];
  const off = (el) => /^(password|file|hidden|submit|button|reset|image)$/.test(el.type) || el.closest('[autocomplete=off]');
  const box = (el) => /^(checkbox|radio)$/.test(el.type);
  function save(k) {
    const d = { f: [], s: {} };
    pend = null;
    fields().forEach((el, i) => {
      const o = el.options && [...el.options];
      if (off(el) || !(o ? o.some((x) => x.selected != x.defaultSelected) : box(el) ? el.checked != el.defaultChecked : el.value != el.defaultValue)) return;
      d.f.push([i, el.name + el.type, o ? o.filter((x) => x.selected).map((x) => x.value) : box(el) ? el.checked : el.value]);
    });
    send('wisp:capture', d);
    try {
      if (d.f.length || Object.keys(d.s).length) sessionStorage.setItem('wisp:' + k, JSON.stringify(d));
      else sessionStorage.removeItem('wisp:' + k);
    } catch {}
  }
  function restore(k) {
    let d;
    try {
      d = JSON.parse(sessionStorage.getItem('wisp:' + k));
    } catch {}
    pend = d?.s;
    const all = fields();
    for (const [i, name, v] of d?.f || []) {
      const el = all[i];
      if (!el || el.name + el.type != name || off(el)) continue;
      if (el.options) for (const o of el.options) o.selected = v.includes(o.value);
      else if (box(el)) el.checked = v;
      else el.value = v;
      el.dispatchEvent(new Event(el.options || box(el) ? 'change' : 'input', { bubbles: true }));
    }
  }
  document.addEventListener('wisp:restore', (e) => (e.detail.s = pend));
  entry ? restore(entry) : (entry = mark());

  document.addEventListener('wisp:goto', (e) => go(e.detail.url, e.detail).finally(e.detail.done));
  document.addEventListener('wisp:refresh', (e) => refresh().finally(e.detail?.done));
  // Browser code failed to start: the route's error page, from the server.
  document.addEventListener('wisp:error', () => refresh({ 'x-wisp-error': '1' }));

  // ---- morph ----------------------------------------------------------------

  // Elements match by tag and id; text and comments by type.
  const same = (a, b) =>
    a.nodeType === b.nodeType && a.nodeName === b.nodeName && (a.nodeType !== 1 || a.id === b.id);

  function morph(a, b) {
    if (a.nodeType !== 1) {
      if (a.nodeValue !== b.nodeValue) a.nodeValue = b.nodeValue;
      return;
    }
    if (a.hasAttribute('data-wisp-keep')) return;
    // Attributes. Setting value/checked/selected attributes only moves the
    // live state if the user has not changed it, which is what we want.
    for (let i = a.attributes.length - 1; i >= 0; i--) {
      const name = a.attributes[i].name;
      if (!b.hasAttribute(name)) a.removeAttribute(name);
    }
    for (const { name, value } of b.attributes) {
      if (a.getAttribute(name) !== value) a.setAttribute(name, value);
    }
    if (a.nodeName === 'TEMPLATE') children(a.content, b.content);
    children(a, b);
  }

  function children(a, b) {
    let ids = null; // the old children by id, made once a new one has an id
    const skip = (c) => {
      while (c && c.__w) c = c.nextSibling;
      return c;
    };
    let cur = skip(a.firstChild);
    for (let next = b.firstChild; next; ) {
      const n = next;
      next = n.nextSibling; // n may move out of b below
      let m = null;
      if (n.nodeType === 1 && n.id) {
        ids ||= new Map([...a.childNodes].filter((c) => c.id && !c.__w).reverse().map((c) => [c.id, c]));
        m = ids.get(n.id);
        if (m?.nodeName !== n.nodeName) m = null;
        else ids.delete(n.id);
      } else if (cur && same(cur, n)) {
        m = cur;
      }
      if (!m) {
        a.insertBefore(n, cur); // new node, adopted from the parsed document
        continue;
      }
      if (m === cur) cur = skip(cur.nextSibling);
      else a.insertBefore(m, cur); // keyed node moved into place
      morph(m, n);
    }
    while (cur) {
      const gone = cur;
      cur = skip(cur.nextSibling);
      gone.remove();
    }
  }

  // dev{
  // `wisp dev`, after a template's text changed: the page again, morphed in
  // only between that file's marks (<!--w:file--> and <!--/w:file-->, which
  // templates write under `wisp dev`), so the rest of the page is left as
  // it is. Marks that differ from the page's, or are not siblings, mean the
  // whole page instead.
  document.addEventListener('wisp:region', async (e) => {
    const { files, done } = e.detail;
    try {
      const res = await fetch(location.href, { headers });
      const to = redirect(res);
      if (to) return void (await go(to, { replace: true }));
      const html = await res.text();
      const doc = answers(new DOMParser().parseFromString(html, 'text/html'));
      const marks = (root) => {
        const out = [];
        const w = document.createTreeWalker(root, NodeFilter.SHOW_COMMENT);
        while (w.nextNode()) if (/^\/?w:/.test(w.currentNode.data)) out.push(w.currentNode);
        return out;
      };
      const [old, now] = [marks(document.body), marks(doc.body)];
      const ok = old.length == now.length && old.every((m, k) => m.data == now[k].data);
      if (!ok) return swap(html, res.status);
      for (let k = 0; k < old.length; k++) {
        if (!files.includes(old[k].data.slice(2))) continue;
        // Its end: the next of its file at the same depth.
        let d = 0, j = k;
        for (; j < old.length; j++) {
          if (old[j].data.slice(old[j].data.indexOf(':') + 1) != old[k].data.slice(2)) continue;
          d += old[j].data[0] == '/' ? -1 : 1;
          if (!d) break;
        }
        const [a, b, c, z] = [old[k], old[j], now[k], now[j]];
        if (!b || a.parentNode !== b.parentNode || c.parentNode !== z.parentNode) return swap(html, res.status);
        let cur = a.nextSibling;
        const skip = () => {
          while (cur !== b && cur.__w) cur = cur.nextSibling;
        };
        skip();
        for (let n = c.nextSibling; n !== z; ) {
          const next = n.nextSibling;
          if (cur !== b && same(cur, n)) {
            morph(cur, n);
            cur = cur.nextSibling;
            skip();
          } else b.parentNode.insertBefore(n, cur);
          n = next;
        }
        while (cur !== b) {
          const gone = cur;
          cur = cur.nextSibling;
          skip();
          gone.remove();
        }
        k = j;
      }
      if (doc.title) document.title = doc.title;
      const json = doc.getElementById('wisp-live');
      const mine = document.getElementById('wisp-live');
      if (json && mine) mine.textContent = json.textContent;
      wake();
      send('wisp:update', { status: res.status });
    } finally {
      done?.();
    }
  });
  // }dev

  // ---- forms ----------------------------------------------------------------

  // A field named `action`, `reset` or `submit` hides the form's property of
  // that name, so forms are read through attributes and the prototype.
  const form_ = HTMLFormElement.prototype;
  const busy = new WeakSet(); // forms with a post out
  let native = null; // a form handed back to the browser

  document.addEventListener('submit', async (e) => {
    const form = e.target;
    const btn = e.submitter;
    // The button's formaction, formmethod... win over the form's own.
    const attr = (name) => btn?.getAttribute('form' + name) ?? form.getAttribute(name);
    const target = attr('target');
    if (
      e.defaultPrevented ||
      form === native ||
      (attr('method') || 'get').toLowerCase() !== 'post' ||
      form.getAttribute('data-wisp') === 'off' ||
      (target && target !== '_self')
    ) return;
    e.preventDefault();
    if (busy.has(form)) return; // Enter pressed again while the post is out

    const url = new URL(attr('action') ?? '', location.href);
    // Sent as the browser would: files only with the multipart enctype.
    const data = new FormData(form, btn);
    if (!send('wisp:submit', { data, submitter: btn, action: url }, form)) return;
    const multipart = (attr('enctype') || '').toLowerCase() === 'multipart/form-data';
    const body = multipart ? data : new URLSearchParams([...data].map(([k, v]) => [k, typeof v === 'string' ? v : v.name]));
    if (!navigator.onLine) {
      // `data-wisp-queue` says sending it twice is safe (the server takes the
      // same post once): it waits for the network. Any other form says so.
      const text = typeof body === 'string' || body instanceof URLSearchParams;
      if (form.hasAttribute('data-wisp-queue') && text) {
        enqueue([url.href, String(body)]);
        say('Saved: it is sent when you are back online.');
      } else say('You are offline. Try again when you are back.');
      send('wisp:result', { ok: false, status: 0, error: 'offline' }, form);
      return;
    }
    busy.add(form);
    form.setAttribute('aria-busy', 'true');
    if (btn) btn.disabled = true;
    let res, html, to;
    const result = { ok: false, status: 0 };
    try {
      res = await fetch(url, { method: 'POST', body, headers });
      Object.assign(result, { ok: res.ok, status: res.status });
      to = redirect(res);
      if (to) result.location = to.href;
      const type = res.headers.get('content-type') || '';
      if (to) {
        // Another page: navigate there. The same one: fetch it below.
        if (to.origin !== location.origin || to.pathname !== location.pathname) res = null;
        else {
          push(to);
          scrollTo(0, 0);
          if (!res.redirected) res = await fetch(to, { headers });
        }
      } else if (type.includes('json') && form.__wispEnhance) {
        result.data = await res.json(); // an action's answer, for use:enhance
        res = null;
      } else if (!isHtml(res)) {
        // Not a page (a file): shown, or saved when the server says so,
        // as the browser would. A blob's address is this site's, so a file
        // to save is never opened as a page here, where its script would run.
        const blob = await res.blob();
        if (!blob.size) await refresh();
        else if (!attachment(res)) location.assign(URL.createObjectURL(blob));
        else {
          const a = document.createElement('a');
          a.href = URL.createObjectURL(blob);
          a.download = /filename="?([^";]*)/i.exec(res.headers.get('content-disposition'))?.[1] || '';
          a.click();
          setTimeout(() => URL.revokeObjectURL(a.href), 60000);
        }
        return;
      }
      if (res) html = await res.text();
    } catch (err) {
      result.error = err;
      res = undefined;
    } finally {
      // Before the morph, so the new page decides what is busy or disabled.
      busy.delete(form);
      form.removeAttribute('aria-busy');
      if (btn) btn.disabled = false;
    }
    if (res === undefined && !form.__wispEnhance) {
      // Network trouble: let the browser post it, and show what went wrong.
      native = form;
      form_.requestSubmit.call(form, btn);
      native = null;
      return;
    }
    if (result.ok && !result.data) form_.reset.call(form); // fields fall back to the server's new defaults
    if (html != null) swap(html, res.status);
    else if (to && !result.data) await go(to, { replace: false });
    send('wisp:result', result, form);
  });

  // Posts that waited for the network: [url, urlencoded body], in
  // sessionStorage (a reload keeps them). Sent in order when online; one
  // the server turns down (4xx) is dropped, a failure keeps the rest.
  const queue = () => {
    try { return JSON.parse(sessionStorage['wisp:q'] || '[]'); } catch { return []; }
  };
  const enqueue = (p) => {
    try { sessionStorage['wisp:q'] = JSON.stringify([...queue(), p]); } catch {}
  };
  async function drain() {
    if (!queue().length) return;
    for (let q; (q = queue()).length && navigator.onLine; ) {
      try {
        const r = await fetch(q[0][0], { method: 'POST', body: q[0][1], headers: { ...headers, 'content-type': 'application/x-www-form-urlencoded' } });
        if (r.status >= 500) return;
      } catch { return; }
      sessionStorage['wisp:q'] = JSON.stringify(q.slice(1));
    }
    send('wisp:sent');
    refresh().catch(() => {});
  }
  addEventListener('online', drain);
  if (queue().length) drain();

  // ---- islands --------------------------------------------------------------

  // A component marked client:visible, client:idle, client:media or
  // client:interaction starts when that happens: only then do its module,
  // and live.js on a page with nothing else to start, load. Until then the
  // server's HTML is all there is, and it works as HTML.
  const runtime = me?.replace('wisp.js', 'live.js');
  let woke; // ends the last page's waits

  function wake() {
    woke?.abort();
    woke = new AbortController();
    const { signal } = woke;
    // Parsed here once: live.js takes it from `__j`. Each record is
    // [I, module, parent, blob, how?] (protocol.rs).
    const el = document.getElementById('wisp-live');
    const json = el && (el.__j = JSON.parse(el.textContent));
    const parent = {};
    const waits = new Map(); // client:interaction islands -> their start
    for (const [I, , P, , how] of json && runtime ? json.i : []) {
      parent[I] = P;
      if (!how) continue;
      let started;
      const go = () =>
        signal.aborted ||
        (started ||= import(runtime)
          .then((m) => signal.aborted || m.hydrate(I))
          .finally(() => waits.delete(I)));
      const idle = () => (window.requestIdleCallback || setTimeout)(go, { timeout: 2000 });
      // What it shows: a hole's anchor is a <template>, so its parent.
      const els = [...document.querySelectorAll(`[data-w^="${I}."]`)].map((el) => (el.content ? el.parentElement : el));
      if (how == 'x') waits.set(I, go);
      else if (how == 'v' && els.length) {
        const io = new IntersectionObserver((es) => es.some((e) => e.isIntersecting) && (io.disconnect(), go()), { rootMargin: '200px' });
        els.forEach((el) => io.observe(el));
        signal.addEventListener('abort', () => io.disconnect());
      } else if (how[0] == 'm') {
        const mq = matchMedia(how.slice(1));
        if (mq.matches) go();
        else mq.addEventListener('change', () => mq.matches && go(), { signal });
      } else idle();
    }
    // The island of the element an event is on, if it waits.
    const island = (t) => {
      for (let el = t.closest?.('[data-w]'); el; el = el.parentElement?.closest('[data-w]')) {
        const [i, g] = el.dataset.w.split('.');
        if (g == null) continue;
        for (let I = +i; I > -1; I = parent[I] ?? -1) if (waits.has(I)) return waits.get(I);
        return;
      }
    };
    // The first pointer, focus or key in one starts it. A click that comes
    // before it is ready is held, and clicked again once it is.
    if (waits.size) for (const type of ['pointerdown', 'focusin', 'keydown', 'click']) {
      document.addEventListener(type, (e) => {
        const go = waits.size && island(e.target);
        if (!go) return;
        const ready = go();
        if (type != 'click') return;
        e.preventDefault();
        e.stopImmediatePropagation();
        ready.then(() => e.target.dispatchEvent(new MouseEvent('click', e)));
      }, { capture: true, signal });
    }
  }
  wake();
  // Opened at a path the host answered with index.html: that path's page.
  if (document.getElementById('wisp-spa'))
    drawn(document.documentElement.outerHTML, location).then((html) => html && swap(html));
})();
