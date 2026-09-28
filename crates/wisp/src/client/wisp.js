// wisp.js: progressive enhancement for Wisp pages. No dependencies.
//
// A <form method="post"> is sent with fetch and the page the server answers
// with is morphed into the current one: nodes are reused where they match,
// so focus, scroll, <details> state and unsaved input elsewhere survive.
// Without this script the same form works as a normal post.
//
// Dispatching `wisp:refresh` on the document morphs in the current URL's
// page again; `wisp dev` does that after every rebuild.
(() => {
  let shown = location.href.split('#')[0];

  function swap(html) {
    const doc = new DOMParser().parseFromString(html, 'text/html');
    if (doc.title) document.title = doc.title;
    morph(document.body, doc.body);
    shown = location.href.split('#')[0];
  }

  async function refresh() {
    const res = await fetch(location.href, { headers: { 'x-wisp': '1' } });
    swap(await res.text());
  }

  // ---- morph ----------------------------------------------------------------

  // Elements match by tag and id; text and comments by type.
  const same = (a, b) =>
    a.nodeType === b.nodeType && a.nodeName === b.nodeName && (a.nodeType !== 1 || a.id === b.id);

  function morph(a, b) {
    if (a.nodeType !== 1) {
      if (a.nodeValue !== b.nodeValue) a.nodeValue = b.nodeValue;
      return;
    }
    // Attributes. Setting value/checked/selected attributes only moves the
    // live state if the user has not changed it, which is what we want.
    for (let i = a.attributes.length - 1; i >= 0; i--) {
      const name = a.attributes[i].name;
      if (!b.hasAttribute(name)) a.removeAttribute(name);
    }
    for (const { name, value } of b.attributes) {
      if (a.getAttribute(name) !== value) a.setAttribute(name, value);
    }
    children(a, b);
  }

  function children(a, b) {
    let cur = a.firstChild;
    for (let next = b.firstChild; next; ) {
      const n = next;
      next = n.nextSibling; // n may move out of b below
      let m = null;
      if (n.nodeType === 1 && n.id) {
        for (let c = cur; c; c = c.nextSibling) if (c.id === n.id && c.nodeName === n.nodeName) { m = c; break; }
      } else if (cur && same(cur, n)) {
        m = cur;
      }
      if (!m) {
        a.insertBefore(n, cur); // new node, adopted from the parsed document
        continue;
      }
      if (m === cur) cur = cur.nextSibling;
      else a.insertBefore(m, cur); // keyed node moved into place
      morph(m, n);
    }
    while (cur) {
      const gone = cur;
      cur = cur.nextSibling;
      gone.remove();
    }
  }

  // ---- forms ----------------------------------------------------------------

  document.addEventListener('submit', async (e) => {
    const form = e.target;
    const btn = e.submitter;
    const method = (btn?.getAttribute('formmethod') || form.getAttribute('method') || 'get').toLowerCase();
    if (
      e.defaultPrevented ||
      method !== 'post' ||
      form.dataset.wisp === 'off' ||
      form.enctype === 'multipart/form-data' ||
      (form.target && form.target !== '_self')
    ) return;
    e.preventDefault();

    const url = btn?.hasAttribute('formaction') ? btn.formAction : form.action;
    const body = new URLSearchParams(new FormData(form, btn));
    form.setAttribute('aria-busy', 'true');
    if (btn) btn.disabled = true;
    let res, html;
    try {
      res = await fetch(url, { method: 'POST', body, headers: { 'x-wisp': '1' } });
      html = await res.text();
    } catch {
      return form.submit(); // network trouble: let the browser do a normal post
    } finally {
      // Before the morph, so the new page decides what is busy or disabled.
      form.removeAttribute('aria-busy');
      if (btn) btn.disabled = false;
    }
    if (res.redirected) {
      history.pushState(null, '', res.url);
      scrollTo(0, 0);
    }
    if (res.ok) form.reset(); // fields fall back to the server's new defaults
    swap(html);
  });

  // Back/forward across entries we pushed: show that URL's page.
  addEventListener('popstate', () => {
    if (location.href.split('#')[0] !== shown) refresh();
  });

  document.addEventListener('wisp:refresh', refresh);
})();
