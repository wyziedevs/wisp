// more.js: small helpers of Wisp's browser runtime, served as
// /_app/c/more.js to the modules whose code names one (see `codegen`), so
// an app that names none pays nothing for them, not even in its binary:
// announce, optimistic, and the actions outside, inview, shortcut, modal,
// preload and keepscroll. It adds them to live.js's `__wisp` helpers.
import { __wisp as X } from 'wisp';

// announce(text): screen readers say it, from a polite live region.
let said;
X.shared.announce = (t) => {
  if (!said?.isConnected) {
    said = document.createElement('div');
    said.setAttribute('aria-live', 'polite');
    said.style.cssText = 'position:absolute;width:1px;height:1px;overflow:hidden;clip-path:inset(50%)';
    said.__w = 1;
    document.body.append(said);
  }
  said.textContent = '';
  setTimeout(() => (said.textContent = String(t)), 50);
};

// optimistic(list, item): item is in the $state list now; the function it
// returns, given the action's result (use:enhance's), takes it out again
// if the action failed: `use:enhance="() => optimistic(todos, { text })"`.
X.shared.optimistic = (list, item) => {
  list.push(item);
  const p = list.at(-1);
  return (r) => {
    const i = r?.ok ? -1 : list.indexOf(p);
    if (i >= 0) list.splice(i, 1);
  };
};

// Actions. An action's argument may change: `update` takes the new one;
// what `go` listens to ends with it.
const act = (f, go) => {
  const v = [f];
  const ac = new AbortController();
  go(v, ac.signal);
  return { update: (w) => (v[0] = w), destroy: () => ac.abort() };
};

// use:outside="f": f(event) at a press outside the element (a menu closing).
X.shared.outside = (el, f) =>
  act(f, (v, signal) => document.addEventListener('pointerdown', (e) => el.contains(e.target) || v[0]?.(e), { capture: true, signal }));

// use:inview="f": f(true) as the element comes into view, f(false) as it leaves.
X.shared.inview = (el, f) =>
  act(f, (v, signal) => {
    if (!globalThis.IntersectionObserver) return;
    const io = new IntersectionObserver((es) => es.forEach((e) => v[0]?.(e.isIntersecting)));
    io.observe(el);
    signal.addEventListener('abort', () => io.disconnect());
  });

// use:preload on a link or around links: each is fetched ahead once in view;
// none with data saver on or under data-wisp-preload="off", as wisp.js's own.
X.shared.preload = (el) =>
  act(0, (v, signal) => {
    if (!globalThis.IntersectionObserver || navigator.connection?.saveData) return;
    const io = new IntersectionObserver((es) =>
      es.forEach((e) => e.isIntersecting && (io.unobserve(e.target), document.dispatchEvent(new CustomEvent('wisp:preload', { detail: { url: e.target.href } })))),
    );
    (el.matches('a[href]') ? [el] : el.querySelectorAll('a[href]')).forEach((a) => a.closest('[data-wisp-preload="off"]') || io.observe(a));
    signal.addEventListener('abort', () => io.disconnect());
  });

// use:keepscroll on a scrolling element: back and forward put its scroll
// back, as the page's. Kept in the history entry by its id (or its place
// among them), beside wisp.js's own.
const kept = [];
const scrolls = () => history.state?.s || {};
let popped = 0;
document.addEventListener('wisp:navigate', (e) => {
  if ((popped = e.detail.pop) || !kept.length) return;
  const s = scrolls();
  kept.forEach((el, i) => (s[el.id || i] = [el.scrollLeft, el.scrollTop]));
  history.replaceState({ ...history.state, s }, '');
});
const put = (el, i) => {
  const at = scrolls()[el.id || i];
  if (at) el.scrollTo(at[0], at[1]);
};
// After back or forward only: a form's morph leaves the scroll be.
document.addEventListener('wisp:update', () => popped && ((popped = 0), kept.forEach(put)));
document.addEventListener('wisp:pop', () => kept.forEach(put));
X.shared.keepscroll = (el) =>
  act(0, (v, signal) => {
    kept.push(el);
    put(el, kept.length - 1);
    signal.addEventListener('abort', () => kept.splice(kept.indexOf(el), 1));
  });

// use:shortcut="'ctrl+k'": the keys click the element (a field: focus it).
// Modifiers ctrl, shift, alt and meta, or mod (meta on a Mac, else ctrl).
// A key typed in a field with no ctrl, alt or meta is the field's own.
const typing = (t) => t?.isContentEditable || t?.matches?.('input,textarea,select');
X.shared.shortcut = (el, k) =>
  act(k, (v, signal) =>
    document.addEventListener(
      'keydown',
      (e) => {
        if (!(e.ctrlKey || e.altKey || e.metaKey) && typing(e.target)) return;
        const p = String(v[0] || '').toLowerCase().split('+');
        const mac = /Mac|iP/.test(navigator.platform);
        const want = (m) => p.includes(m) || (p.includes('mod') && m == (mac ? 'meta' : 'ctrl'));
        if (e.key?.toLowerCase() != p.at(-1) || !['ctrl', 'shift', 'alt', 'meta'].every((m) => want(m) == e[m + 'Key'])) return;
        e.preventDefault();
        el.matches('input,textarea,select,[contenteditable]') ? el.focus() : el.click();
      },
      { signal },
    ),
  );

// use:modal="open" on a <dialog>: shown as a modal while open is true.
// On a variable the compiler hands `[open, set]`: every close (Escape, a
// method=dialog form, close()) sets it false, so true opens it again.
X.shared.modal = (el, o) => {
  let to;
  const shut = () => to?.(false);
  el.addEventListener('close', shut);
  const set = (o) => {
    if (Array.isArray(o)) [o, to] = o;
    o ? el.open || el.showModal?.() : el.open && el.close();
  };
  set(o);
  return { update: set, destroy: () => (el.removeEventListener('close', shut), el.open && el.close()) };
};
