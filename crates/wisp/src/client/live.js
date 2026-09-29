// live.js: the browser half of Wisp's client code. No dependencies.
//
// The server renders the whole page; this makes the parts written with
// browser code (on:, bind:, :attr, {:expr}, {:#each}, ...) react in the
// browser. A .wisp file with browser code compiles to a module that calls
// define() with a function returning its binding groups. The server tags
// each directive element data-w="instance.group" (data-wl: the Rust loop
// values it uses) and lists the instances in the #wisp-live JSON.
//
// No dependency tracking: after anything that may change state (a handler,
// an input, a timer, a store) every binding is evaluated again in one
// batch, and the DOM is written only where a value changed.
//
// A morph (a form post, a navigation) keeps an instance whose elements
// survive it: its state stays, and it receives the new server values. An
// element with data-wisp-reset, or inside one, starts its instances afresh.
// Nodes made here are marked __w, so the morph in wisp.js leaves them.

const defs = new Map(); // module id -> { fn, html, load }
const urls = new Map(); // module id -> the url it was loaded from
const seen = new WeakSet(); // elements bound before: a restart does not animate them in
const trans = new WeakMap(); // element -> [name, options] from transition:
const anims = new WeakMap(); // element -> its running animation
const flips = new WeakSet(); // elements with animate:flip
const recs = new WeakMap(); // element -> its binding record { inst, key, sc, el }
const toggled = new WeakMap(); // element -> Map of the classes class: set, which a live class attribute keeps
const effects = new Set();
const hooks = []; // run after every redraw (persisted stores save)
const reduce = matchMedia('(prefers-reduced-motion: reduce)');
const FLAGS = 'prevent stop once self capture passive window document outside debounce ctrl shift alt meta'.split(' ');
const KEYS = { space: ' ', up: 'arrowup', down: 'arrowdown', left: 'arrowleft', right: 'arrowright' };
const NONE = {};
let live = []; // the instances the server rendered, in render order
let mounts = []; // [instance, onMount callback] waiting for their first redraw
let queued = null; // the pending redraw
let loaded = false; // nothing animates in on the page's first load
let gen = 0; // bumped per start, so a slow import cannot boot a stale page
let epoch = 0; // bumped per start: a morph may have rewritten what bindings wrote
let erred = false; // an error page was asked for since the last navigation

export function define(id, fn, opts) {
  defs.set(id, { fn, ...opts });
}

// ---- stores ---------------------------------------------------------------

// A value any code can share: `cart.value` reads it, setting it redraws
// every instance. Lives as long as the page, so across morphs and
// navigations; put one in a src/lib module to share it between files.
export function store(value) {
  const subs = new Set();
  const s = {
    get value() {
      return value;
    },
    set value(v) {
      value = v;
      subs.forEach((f) => f(v));
      redraw();
    },
    set: (v) => (s.value = v),
    update: (f) => (s.value = f(value)),
    subscribe(f) {
      subs.add(f);
      f(value);
      return () => subs.delete(f);
    },
  };
  return s;
}

// A store kept in localStorage under `key`, and in step across tabs. It is
// saved after every redraw it changed in, so changing it in place works.
// One per key: a script that asks again (each time it starts) gets the same.
const saved = new Map();
export function persisted(key, initial) {
  if (saved.has(key)) return saved.get(key);
  const load = (j) => {
    try {
      return j == null ? initial : JSON.parse(j);
    } catch {
      return initial;
    }
  };
  let json = null;
  try {
    json = localStorage.getItem(key);
  } catch {}
  const s = store(load(json));
  hooks.push(() => {
    const j = JSON.stringify(s.value);
    if (j === json) return;
    json = j;
    try {
      localStorage.setItem(key, j);
    } catch {}
  });
  addEventListener('storage', (e) => e.key === key && (s.value = load((json = e.newValue))));
  saved.set(key, s);
  return s;
}

// A value worked out from others, read as `total.value` like a store's.
export function derived(f) {
  return { get value() { return f(); }, subscribe: (g) => (g(f()), () => {}) };
}

export const page = store({ url: new URL(location.href), status: 200, form: undefined });
export const navigating = store(null);

// Client navigation, done by wisp.js: to `url`, or the current page again.
export function goto(url, opts = {}) {
  return new Promise((done) => send('wisp:goto', { url: String(url), replace: !!opts.replace, done }));
}

export function invalidate() {
  return new Promise((done) => send('wisp:refresh', { done }));
}

export const tick = () => redraw();

function send(type, detail) {
  document.dispatchEvent(new CustomEvent(type, { detail }));
}

// ---- redraw ---------------------------------------------------------------

// One failing binding (or cleanup) logs and leaves the rest to run. Each
// error is logged once per binding: redraws are frequent.
const failed = new WeakMap();
function run(fns) {
  for (const f of fns) {
    try {
      f();
    } catch (e) {
      if (failed.get(f) !== String(e)) console.error(e);
      failed.set(f, String(e));
    }
  }
}

function redraw() {
  return (queued ||= Promise.resolve().then(() => {
    queued = null;
    for (const inst of live) draw(inst);
    after();
  }));
}

function draw(inst) {
  run(inst.sc.draws);
  for (const r of inst.recs) run(r.sc.draws);
}

// Effects whose inputs changed, then what waited for its first redraw.
function after() {
  for (const e of effects) {
    try {
      let d;
      if (e.deps) {
        d = e.deps();
        if (e.last !== NONE && same(d, e.last)) continue;
        e.last = d;
      }
      e.done?.();
      const r = e.fn(d);
      e.done = typeof r == 'function' ? r : null;
      // Only an effect with inputs redraws: it runs again only when they change.
      if (e.deps) redraw();
    } catch (err) {
      console.error(err);
    }
  }
  const now = mounts;
  mounts = [];
  for (const [inst, f] of now) {
    if (inst.sc.signal.aborted) continue;
    fire(() => {
      const r = f();
      if (typeof r == 'function') inst.sc.stops.push(r);
    });
  }
  run(hooks);
}

const same = (a, b) =>
  Array.isArray(a) && Array.isArray(b) ? a.length == b.length && a.every((x, i) => Object.is(x, b[i])) : Object.is(a, b);

// Runs user code that may change state, then redraws; a returned promise
// redraws again when it settles. Errors still throw, to reach the console.
function fire(f, ...args) {
  try {
    const r = f(...args);
    if (typeof r?.then == 'function') Promise.resolve(r).finally(redraw);
    return r;
  } finally {
    redraw();
  }
}

// A draw that calls f only when get's value changed, or when a start came
// between (a morph may have rewritten the DOM). `first` is true then too.
function watch(get, L, f, again = true) {
  let last = NONE;
  let ep = epoch;
  return () => {
    const v = get(L);
    if (Object.is(v, last) && (ep === epoch || !again)) return;
    const first = last === NONE || ep !== epoch;
    ep = epoch;
    f((last = v), first);
  };
}

// A scope owns draws and cleanups: one per instance, element and clone.
function scope() {
  const ac = new AbortController();
  return { draws: [], stops: [() => ac.abort()], signal: ac.signal };
}

// ---- start ----------------------------------------------------------------

// Matches the instances in #wisp-live with the running ones, after the
// first load and after every morph.
function start() {
  const my = ++gen;
  const json = document.getElementById('wisp-live');
  const { m = {}, i = [] } = json ? JSON.parse(json.textContent) : {};
  const need = Object.entries(m).filter(([id, url]) => urls.get(id) !== url);
  if (!need.length) return boot(i, my); // no await: restart before the next paint
  Promise.all(
    need.map(([id, url]) => (urls.set(id, url), import(url).catch((e) => console.error(e))))
  ).then(() => my === gen && boot(i, my));
}

function boot(list, my) {
  epoch++;
  const els = {};
  for (const el of document.querySelectorAll('[data-w]')) {
    const [i, g] = el.dataset.w.split('.');
    if (g != null) (els[i] ||= []).push(el);
  }
  const old = live;
  const byI = {};
  const waiting = {}; // instance -> what starts once its +page.js has loaded
  const kept = new Set();
  live = [];
  const begin = ([I, id, P, blob]) => {
    const def = defs.get(id);
    const mine = els[I] || [];
    if (!def) return;
    // Its parent's +page.js is still loading: start after it, so that
    // getContext finds what the parent's script sets.
    if (waiting[P]) return waiting[P].push([I, id, P, blob]);
    // Kept: the instance whose element this is, or failing that (its
    // elements were all replaced) the one at the same place in the list.
    let inst = null;
    if (!mine.some((el) => el.closest('[data-wisp-reset]'))) {
      const ok = (o) => o && o.def === def && !kept.has(o) && old.includes(o) && !o.sc.signal.aborted;
      inst = mine.map((el) => recs.get(el)?.inst).find(ok) || old.find((o) => o.I === I && ok(o));
    }
    const parent = byI[P] || null;
    if (inst) {
      kept.add(inst);
      inst.I = I;
      inst.parent = parent;
      live.push(inst);
      bindAll(inst, mine);
      if (inst.s) loadThen(def, blob, my, (b) => inst.s(b));
    } else if (def.load) {
      // Its +page.js loads first: the page's elements wait unbound.
      waiting[I] = [];
      loadThen(def, blob, my, (b) => {
        const late = create(def, id, b, mine[0], parent);
        late.I = I;
        live.push(late);
        bindAll(late, mine);
        byI[I] = late;
        const next = waiting[I];
        delete waiting[I];
        next.forEach(begin);
      });
    } else {
      inst = create(def, id, blob, mine[0], parent);
      inst.I = I;
      live.push(inst);
      bindAll(inst, mine);
    }
    if (inst) byI[I] = inst;
  };
  list.forEach(begin);
  for (const o of old) if (!kept.has(o)) destroy(o);
  loaded = true;
  redraw();
}

// Runs the module's +page.js `load` if it has one, then f with the values
// it gives as `data`. A failing load shows the error page.
function loadThen(def, blob, my, f) {
  if (!def.load) return fire(f, blob);
  const url = new URL(location.href);
  Promise.resolve()
    .then(() => def.load({ data: blob.data, url, fetch }))
    .then((data) => my === gen && fire(f, { ...blob, data }), boundary);
}

// Runs a module's script for a new instance. A script that throws while
// starting shows the route's error page.
function create(def, id, blob, el, parent) {
  const inst = { def, id, I: -1, sc: scope(), recs: new Set(), ctx: new Map(), el, parent, g: [], events: {} };
  try {
    const r = def.fn(blob, helpers(inst));
    inst.g = r.g;
    inst.s = r.s;
    inst.p = r.p;
  } catch (e) {
    boundary(e);
  }
  return inst;
}

function destroy(inst) {
  for (const r of inst.recs) stopRec(r);
  run(inst.sc.stops);
}

// Binds each element to its group, keeping the binding of an element that
// is still marked the same, and drops the bindings of elements now gone.
function bindAll(inst, elements) {
  const keep = new Set();
  for (const el of elements) {
    const key = el.dataset.w + ' ' + (el.dataset.wl || '');
    let r = recs.get(el);
    if (!(r && r.inst === inst && r.key === key && inst.recs.has(r))) {
      if (r) stopRec(r);
      r = { inst, key, sc: scope(), el };
      recs.set(el, r);
      inst.recs.add(r);
      setup(r.sc, inst, el, +el.dataset.w.split('.')[1], JSON.parse(el.dataset.wl || '{}'), !loaded || seen.has(el));
      seen.add(el);
    }
    keep.add(r);
  }
  for (const r of inst.recs) if (!keep.has(r)) stopRec(r);
}

function stopRec(r) {
  run(r.sc.stops);
  r.inst.recs.delete(r);
  if (recs.get(r.el) === r) recs.delete(r.el);
}

// Asks wisp.js for the route's error page, once per navigation.
function boundary(e) {
  console.error(e);
  if (erred) return;
  erred = true;
  send('wisp:error', { error: e });
}

// What a module's function gets. Timers and listeners end with the
// instance and redraw after each call. A timeout or frame that comes after
// teardown does nothing, so only what repeats has to be stopped.
function helpers(inst) {
  const sc = inst.sc;
  const signal = sc.signal;
  const wrap = (f) => (...a) => signal.aborted || fire(f, ...a);
  return {
    tick,
    setTimeout: (f, ms, ...a) => setTimeout(wrap(f), ms, ...a),
    requestAnimationFrame: (f) => requestAnimationFrame(wrap(f)),
    setInterval(f, ms, ...a) {
      const id = setInterval(wrap(f), ms, ...a);
      sc.stops.push(() => clearInterval(id));
      return id;
    },
    addEventListener: (type, f, options) => addEventListener(type, wrap(f), { ...options, signal }),
    listen(url, f) {
      const es = new EventSource(url);
      es.onmessage = (e) => wrap(f)(e.data, e);
      sc.stops.push(() => es.close());
      return es;
    },
    onMount: (f) => mounts.push([inst, f]),
    onDestroy: (f) => sc.stops.push(f),
    // effect(fn) runs after every redraw; effect(fn, () => [a, b]) when a or
    // b changed. What fn returns runs before the next run and at the end.
    effect(fn, deps) {
      const e = { fn, deps, last: NONE, done: null };
      effects.add(e);
      sc.stops.push(() => (effects.delete(e), e.done?.()));
    },
    derived,
    store,
    persisted,
    emit: (name, value) => inst.events[name]?.(value),
    setContext: (k, v) => inst.ctx.set(k, v),
    getContext(k) {
      for (let i = inst; i; i = i.parent) if (i.ctx.has(k)) return i.ctx.get(k);
    },
    goto,
    invalidate,
    page,
    navigating,
    // use:enhance="fn" on a form: fn({ form, formData, submitter, action, cancel }) runs
    // as it is sent (for pending state and optimistic updates) and may
    // return fn(result), run with { ok, status, data, location } after the
    // page has been updated. `data` is the action's JSON answer, if any.
    enhance(form, f) {
      let after;
      const ac = new AbortController();
      const on = (type, g) => form.addEventListener(type, g, { signal: ac.signal });
      form.__wispEnhance = true;
      on('wisp:submit', (e) => {
        const { data: formData, submitter, action } = e.detail;
        after = typeof f == 'function' && fire(f, { form, formData, submitter, action, cancel: () => e.preventDefault() });
      });
      on('wisp:result', (e) => {
        page.value = { ...page.value, form: e.detail.data };
        Promise.resolve(after).then((g) => typeof g == 'function' && fire(g, e.detail));
      });
      return () => ac.abort();
    },
  };
}

// ---- bindings -------------------------------------------------------------

// Binds group g of inst to el with locals L. A quiet element does not
// animate in.
function setup(sc, inst, el, g, L, quiet) {
  for (const [kind, a, b, c, d] of inst.g[g] || []) {
    try {
      const f = binding(sc, inst, el, L, quiet, kind, a, b, c, d);
      if (f) sc.draws.push(f);
    } catch (e) {
      console.error(e);
    }
  }
  if (!quiet && trans.has(el)) {
    let first = true;
    sc.draws.push(() => {
      if (first && !el.hidden) play(el);
      first = false;
    });
  }
}

// The directive elements of a fragment made here: a clone, or a component.
function bindFrag(frag, sc, inst, L, quiet) {
  for (const el of frag.querySelectorAll('[data-w]')) setup(sc, inst, el, +el.dataset.w, L, quiet);
  for (const el of frag.querySelectorAll('template[data-wslot]')) {
    const s = inst.slot;
    if (s) place(el, s.tpl, sc, s.inst, s.L, quiet);
  }
}

// A copy of tpl's content after `at`, bound to inst: its nodes, from the
// first to an end marker (so what is later put inside the range moves and
// goes with it).
function place(at, tpl, sc, inst, L, quiet) {
  const frag = tpl.content.cloneNode(true);
  frag.append(document.createComment(''));
  const first = frag.firstChild;
  const last = frag.lastChild;
  for (const n of frag.childNodes) n.__w = 1;
  bindFrag(frag, sc, inst, L, quiet);
  at.after(frag);
  return { first, last };
}

function range(c) {
  const out = [];
  for (let n = c.first; n; n = n.nextSibling) {
    out.push(n);
    if (n === c.last) break;
  }
  return out;
}

function binding(sc, inst, el, L, quiet, kind, a, b, c, d) {
  const signal = sc.signal;
  switch (kind) {
    case 'on': {
      const has = (m) => b.includes(m);
      const keys = b.filter((m) => !FLAGS.includes(m) && !/^\d+m?s$/.test(m)).map((k) => KEYS[k] || k);
      const t = has('debounce') && b[b.indexOf('debounce') + 1];
      const ms = !has('debounce') ? 0 : /s$/.test(t || '') ? parseInt(t) * (/ms$/.test(t) ? 1 : 1000) : 250;
      const at = has('window') ? window : has('document') || has('outside') ? document : el;
      let timer, done;
      at.addEventListener(a, (e) => {
        if (has('self') && e.target !== el) return;
        if (has('outside') && e.composedPath().includes(el)) return; // the path survives the target's removal
        if (keys.length && !keys.includes(e.key?.toLowerCase())) return;
        if (['ctrl', 'shift', 'alt', 'meta'].some((k) => has(k) && !e[k + 'Key'])) return;
        // .once by hand, so an event the filters above turn away does not use it up.
        if (done) return;
        done = has('once');
        if (has('prevent')) e.preventDefault();
        if (has('stop')) e.stopPropagation();
        if (!ms) return fire(c, L, e);
        clearTimeout(timer);
        timer = setTimeout(() => signal.aborted || fire(c, L, e), ms);
      }, { capture: has('capture'), passive: has('passive'), signal });
      return;
    }
    case 'bind': {
      if (a == 'this') return void c(L, el);
      const read = () =>
        a == 'checked' ? el.checked : /^(number|range)$/.test(el.type) && el.value !== '' ? +el.value : el.value;
      // What the user typed before this ran wins, then the script's value,
      // then what the page shows.
      const typed = a == 'checked' ? el.checked !== el.defaultChecked : 'defaultValue' in el && el.value !== el.defaultValue;
      if (typed || b(L) == null) c(L, read());
      el.addEventListener(a == 'checked' ? 'change' : 'input', () => fire(c, L, read()), { signal });
      // A form reset (wisp.js resets a form its post succeeded with) moves
      // the field to its new default; the variable follows it.
      el.form?.addEventListener('reset', () => queueMicrotask(() => fire(c, L, read())), { signal });
      // Compared with the element, not the last value: a handler may change
      // the target before the input's own redraw.
      return () => {
        const v = a == 'checked' ? !!b(L) : b(L);
        if (read() === v) return;
        if (a == 'checked') el.checked = v;
        else el.value = v ?? '';
      };
    }
    case 'attr':
      return watch(b, L, (v, first) => {
        // aria-* states are "true" or "false"; elsewhere false means absent.
        const aria = a.startsWith('aria-');
        const s = v == null || (v === false && !aria) ? null : v === true && !aria ? '' : String(v);
        const put = () => {
          if (s == null) el.removeAttribute(a);
          else el.setAttribute(a, s);
          if (a == 'value') el.value = s ?? '';
          else if (a == 'checked' || a == 'selected') el[a] = s != null;
          else if (a == 'class') toggled.get(el)?.forEach((on, name) => el.classList.toggle(name, on));
        };
        // A transition plays as `hidden` turns off, and before it turns on.
        if (a != 'hidden' || first || !trans.has(el)) return put();
        if (s != null) return play(el, true, () => signal.aborted || put());
        put();
        play(el);
      });
    case 'text':
      return watch(a, L, (v) => (el.textContent = v ?? ''));
    case 'hole': {
      // The anchor's value goes in one text node between it and the end
      // comment after it; a morph may have added or taken nodes there.
      let node = null;
      return watch(a, L, (v) => {
        if (!node || node.previousSibling !== el) {
          node = null;
          for (let n = el.nextSibling; n && n.nodeType != 8; ) {
            const next = n.nextSibling;
            if (!node && n.nodeType == 3) node = n;
            else n.remove();
            n = next;
          }
          if (!node) el.after((node = document.createTextNode('')));
        }
        const s = v == null ? '' : String(v);
        if (node.data !== s) node.data = s;
      });
    }
    case 'class':
      return watch(b, L, (v) => {
        const t = toggled.get(el) || new Map();
        toggled.set(el, t.set(a, !!v));
        el.classList.toggle(a, !!v);
      });
    case 'style':
      return watch(b, L, (v) => {
        if (v == null || v === false) el.style.removeProperty(a);
        else el.style.setProperty(a, v);
      });
    case 'transition':
      trans.set(el, [a, () => b?.(L)]);
      return;
    case 'animate':
      flips.add(el);
      return;
    case 'use': {
      let r;
      sc.stops.push(() => (typeof r == 'function' ? r() : r?.destroy?.()));
      return watch(b || (() => {}), L, (v, first) => (first ? (r = a(L)(el, v)) : r?.update?.(v)), false);
    }
    case 'each':
    case 'if':
      return clones(sc, inst, el, L, quiet, kind, a, b || [], c);
    case 'comp':
      return mount(sc, inst, el, L, quiet, a, b, c, d);
  }
}

// {:#each} and {:#if} (and <template each|if>): one copy of the template's
// content per item, after it, kept by key (by index without one) and moved
// into order. Copies' locals inherit the template's, so a nested block
// sees the outer item.
function clones(sc, inst, tpl, L, quiet, kind, get, names, keyOf) {
  let list = [];
  let first = true;
  sc.stops.push(() => list.forEach((c) => (run(c.sc.stops), range(c).forEach((n) => n.remove()))));
  return () => {
    const items = kind == 'if' ? (get(L) ? [0] : []) : [...(get(L) ?? [])];
    const old = new Map(list.map((c) => [c.key, c]));
    // animate:flip: where each element was, to slide it from there.
    const rects = new Map();
    for (const c of list) for (const n of range(c)) if (flips.has(n)) rects.set(n, n.getBoundingClientRect());
    const next = items.map((item, i) => {
      const cl = Object.create(L);
      if (names[0]) cl[names[0]] = item;
      if (names[1]) cl[names[1]] = i;
      const key = keyOf ? keyOf(cl) : i;
      const c = old.get(key);
      if (!c) return { key, L: cl, sc: scope() };
      old.delete(key);
      if (names[0]) c.L[names[0]] = item;
      if (names[1]) c.L[names[1]] = i;
      return c;
    });
    for (const c of old.values()) remove(sc, c);
    let at = tpl;
    for (const c of next) {
      if (!c.first) Object.assign(c, place(at, tpl, c.sc, inst, c.L, quiet && first));
      else if (at.nextSibling !== c.first) at.after(...range(c));
      run(c.sc.draws);
      at = c.last;
    }
    list = next;
    first = false;
    for (const [el, r] of rects) {
      if (!el.isConnected || reduce.matches) continue;
      const now = el.getBoundingClientRect();
      const dx = r.left - now.left;
      const dy = r.top - now.top;
      if (dx || dy) el.animate([{ transform: `translate(${dx}px, ${dy}px)` }, { transform: 'none' }], { duration: 200, easing: 'ease-out' });
    }
  };
}

// A copy leaves: its transitions play out first, if it has any.
function remove(sc, c) {
  run(c.sc.stops);
  const nodes = range(c);
  const gone = () => sc.signal.aborted || nodes.forEach((n) => n.remove());
  const out = nodes.flatMap((n) => (n.nodeType == 1 ? [n, ...n.querySelectorAll('[data-w]')] : [])).filter((el) => trans.has(el));
  let left = out.length;
  if (!left) return gone();
  for (const el of out) play(el, true, () => --left || gone());
}

// A component the browser renders, after its anchor: a new instance of its
// module with the props from the page's code. Changed props go in on every
// redraw; a `bind:` prop the component changed comes back out.
function mount(sc, parent, anchor, L, quiet, id, props, binds, events) {
  const def = defs.get(id);
  const child = { def, id, sc: scope(), recs: new Set(), ctx: new Map(), el: anchor, parent, g: [], events: {} };
  child.slot = { tpl: anchor, inst: parent, L };
  for (const [name, f] of events) child.events[name] = (v) => fire(f, L, v);
  let last = props(L);
  try {
    const r = def.fn({ ...last }, helpers(child));
    Object.assign(child, { g: r.g, s: r.s, p: r.p });
  } catch (e) {
    console.error(e);
  }
  def.tpl ||= Object.assign(document.createElement('template'), { innerHTML: def.html || '' });
  const where = place(anchor, def.tpl, child.sc, child, {}, quiet);
  sc.stops.push(() => (run(child.sc.stops), range(where).forEach((n) => n.remove())));
  return () => {
    if (child.p) {
      const now = child.p();
      for (const [name, , set] of binds) {
        if (!Object.is(now[name], last[name])) fire(set, L, (last[name] = now[name]));
      }
    }
    const p = props(L);
    if (child.s && Object.keys(p).some((k) => !Object.is(p[k], last[k]))) {
      last = { ...last, ...p };
      child.s({ ...child.p(), ...p });
    }
    run(child.sc.draws);
  };
}

// ---- transitions ----------------------------------------------------------

// Plays el's transition in, or out (the same frames reversed); done runs
// when it finishes, not when a newer play on el cancels it.
function play(el, out, done) {
  anims.get(el)?.cancel();
  const t = trans.get(el);
  if (!t) return done?.();
  const o = { duration: 150, ...t[1]() };
  const from = {
    fade: { opacity: 0 },
    scale: { opacity: 0, transform: `scale(${o.start ?? 0.95})` },
    fly: { opacity: 0, transform: `translate(${o.x ?? 0}px, ${o.y ?? 8}px)` },
    slide: { opacity: 0, height: 0, overflow: 'hidden' },
  }[t[0]];
  const to = t[0] == 'slide' ? { height: el.offsetHeight + 'px', overflow: 'hidden' } : {};
  const anim = el.animate([from, to], {
    duration: reduce.matches ? 0 : o.duration,
    easing: o.easing || 'ease-out',
    direction: out ? 'reverse' : 'normal',
  });
  anims.set(el, anim);
  if (done) anim.onfinish = done;
}

// ---- navigation -----------------------------------------------------------

document.addEventListener('wisp:navigate', (e) => {
  erred = false;
  navigating.value = { from: new URL(e.detail.from), to: new URL(e.detail.to, location.href) };
});
document.addEventListener('wisp:update', (e) => {
  page.value = { ...page.value, url: new URL(location.href), status: e.detail?.status ?? 200 };
  navigating.value = null;
  start();
});
start();
