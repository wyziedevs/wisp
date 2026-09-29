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
// Events on the document: `wisp:navigate` before a navigation, `wisp:update`
// after each morph (live.js restarts browser code on it). Dispatching
// `wisp:refresh` morphs in the current URL's page again (`wisp dev` does it
// after every rebuild), `wisp:goto` navigates. A form gets `wisp:submit`
// (cancelable) before it is sent and `wisp:result` after. An element with
// `data-wisp-keep` is left as it is, for a widget that owns its own DOM.
// Nodes that browser code made (marked __w) are left too.
(() => {
  const headers = { 'x-wisp': '1' };
  const key = (u) => String(u).split('#')[0];
  let shown = key(location.href);
  const ran = new Set([...document.scripts].map((s) => s.src || s.text));
  // Head elements the server sent, by their markup: a navigation swaps
  // these, and leaves alone what scripts added.
  let served = new Map([...document.head.children].map((n) => [n.outerHTML, n]));

  function swap(html, status = 200) {
    const doc = new DOMParser().parseFromString(html, 'text/html');
    if (doc.title) document.title = doc.title;
    const next = new Map([...doc.head.children].map((n) => [n.outerHTML, n]));
    for (const [k, n] of served) if (!next.has(k)) n.remove();
    for (const [k, n] of next) if (served.has(k)) next.set(k, served.get(k));
    else document.head.append(n);
    served = next;
    morph(document.body, doc.body);
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
    send('wisp:update', { status });
  }

  const send = (type, detail, at = document) => {
    const e = new CustomEvent(type, { detail, cancelable: true });
    at.dispatchEvent(e);
    return !e.defaultPrevented;
  };

  // Where a response sends the page. The server hands its redirects to us
  // as `x-wisp-location`, since fetch would follow them with the post's own
  // headers, and to another site not at all; fetch follows any other.
  function redirect(res) {
    const to = res.headers.get('x-wisp-location');
    return to ? new URL(to, location.href) : res.redirected ? new URL(res.url) : null;
  }

  const isHtml = (res) => (res.headers.get('content-type') || '').startsWith('text/html');

  async function refresh(extra) {
    const res = await fetch(location.href, { headers: { ...headers, ...extra } });
    const to = redirect(res);
    if (to) return go(to, { replace: true });
    swap(await res.text(), res.status);
  }

  // ---- navigation -----------------------------------------------------------

  let nav = 0; // the latest navigation; an older one that finishes late is dropped
  const pre = new Map(); // url -> [when, response promise], from hovering
  addEventListener('pagehide', () => (history.scrollRestoration = 'auto'));
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

  async function go(url, how = {}) {
    url = new URL(url, location.href);
    if (url.origin !== location.origin) return location.assign(url);
    const my = ++nav;
    if (!how.pop) history.replaceState({ ...history.state, x: scrollX, y: scrollY }, '');
    send('wisp:navigate', { from: location.href, to: url.href });
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
    const html = await res.text();
    if (my !== nav) return;
    if (how.pop);
    else if (how.replace) history.replaceState({}, '', url);
    else history.pushState({}, '', url);
    const show = () => {
      swap(html, res.status);
      const at = how.pop && history.state;
      if (at) scrollTo(at.x || 0, at.y || 0);
      else if (url.hash) document.getElementById(decodeURIComponent(url.hash.slice(1)))?.scrollIntoView();
      else scrollTo(0, 0);
      // Focus starts over, as on a page load, unless the page asks for it.
      const auto = document.querySelector('[autofocus]');
      if (auto) auto.focus();
      else {
        document.activeElement?.blur();
        document.body.setAttribute('tabindex', '-1');
        document.body.focus({ preventScroll: true });
        document.body.removeAttribute('tabindex');
      }
    };
    if (document.startViewTransition && !matchMedia('(prefers-reduced-motion: reduce)').matches) {
      // Its animation is skipped, rejecting these, when the tab is hidden.
      const vt = document.startViewTransition(show);
      vt.ready.catch(() => {});
      vt.finished.catch(() => {});
      await vt.updateCallbackDone;
    } else show();
  }

  document.addEventListener('click', (e) => {
    const a = e.target.closest?.('a[href]');
    if (!a || e.defaultPrevented || e.button || e.metaKey || e.ctrlKey || e.shiftKey || e.altKey || !ours(a)) return;
    const url = new URL(a.href);
    // Same page, another #place (or a bare `#`): the browser scrolls there.
    if (a.href.includes('#') && key(url) === key(location.href)) return;
    e.preventDefault();
    go(url);
  });

  // Fetches a link's page ahead, used if it is followed within 10s.
  function preload(e) {
    const a = e.target.closest?.('a[href]');
    if (!a || !ours(a) || a.closest('[data-wisp-preload="off"]')) return;
    const k = key(a.href);
    if (k === key(location.href) || (pre.has(k) && Date.now() - pre.get(k)[0] < 10000)) return;
    pre.set(k, [Date.now(), fetch(k, { headers }).catch(() => null)]);
  }
  let hover;
  document.addEventListener('mouseover', (e) => {
    clearTimeout(hover);
    hover = setTimeout(() => preload(e), 60);
  });
  document.addEventListener('touchstart', preload, { passive: true });

  // Back/forward across entries we pushed: show that URL's page.
  addEventListener('popstate', () => {
    if (key(location.href) !== shown) go(location.href, { pop: true });
  });

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
        for (let c = cur; c; c = c.nextSibling) if (!c.__w && c.id === n.id && c.nodeName === n.nodeName) { m = c; break; }
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
          history.pushState(null, '', to);
          scrollTo(0, 0);
          if (!res.redirected) res = await fetch(to, { headers });
        }
      } else if (type.includes('json') && form.__wispEnhance) {
        result.data = await res.json(); // an action's answer, for use:enhance
        res = null;
      } else if (!isHtml(res)) {
        // Not a page (a file): show it as the browser would.
        const blob = await res.blob();
        if (blob.size) location.assign(URL.createObjectURL(blob));
        else await refresh();
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
})();
