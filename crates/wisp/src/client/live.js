// live.js: the browser half of Wisp's client code. No dependencies.
//
// The server renders the whole page; this makes the parts written with
// browser code (on:, bind:, :attr, {:expr}, {:#each}, ...) react in the
// browser. A .wisp file with browser code compiles to a module that calls
// define() with a function returning its binding groups. The server tags
// each directive element data-w="instance.group" (data-wl: the Rust loop
// values it uses) and lists the instances in the #wisp-live JSON.
//
// Fine-grained: a script's state is signals (the compiler reads `count` as
// `count.v`), the objects and arrays in it are proxies that track each key,
// and every binding is a node that remembers what it read. A write queues
// only the nodes that read it. They run in one batch, in a microtask,
// parents before children and effects last; a derived value is worked out
// again only when it is read after one of its inputs changed. No virtual
// DOM, and a component's script runs once.
//
// A morph (a form post, a navigation) keeps an instance whose elements
// survive it: its state stays, and it receives the new server values. An
// element with data-wisp-reset, or inside one, starts its instances afresh.
// Nodes made here are marked __w, so the morph in wisp.js leaves them.
//
// Where the server knew a block's or a component's values it painted the
// copies itself, after the anchor, each between <!--[--> and <!--]-->: the
// first draw takes those nodes over instead of making new ones.
//
// An island (a component marked client:visible, client:idle, ...) waits:
// wisp.js calls hydrate() when its moment comes, and only then is its
// module loaded.

const defs = new Map(); // module id -> { fn, html, load }
const done = new Set(); // module urls imported
const seen = new WeakSet(); // elements bound before: a restart does not animate them in
const trans = new WeakMap(); // element -> [name, options] from transition:
const anims = new WeakMap(); // element -> its running animation
const flips = new WeakSet(); // elements with animate:flip
const recs = new WeakMap(); // element -> its binding record { inst, key, sc, el }
const toggled = new WeakMap(); // element -> Map of the classes class: set, which a live class attribute keeps
const reduce = matchMedia('(prefers-reduced-motion: reduce)');
const FLAGS = 'prevent stop once self capture passive window document outside debounce ctrl shift alt meta'.split(' ');
const KEYS = { space: ' ', up: 'arrowup', down: 'arrowdown', left: 'arrowleft', right: 'arrowright' };
const NONE = {};
let live = []; // the instances running, in render order
let route = { id: null, params: {} }; // the page's, for a +page.js load
let mounts = []; // [instance, onMount callback] waiting for the next batch
let loaded = false; // nothing animates in on the page's first load
let gen = 0; // bumped per start, so a slow import cannot boot a stale page
let epoch = 0; // bumped per start: a morph may have rewritten what bindings wrote
let erred = false; // an error page was asked for since the last navigation
let cur = { list: [], at: {}, m: {} }; // the last start's instances, and its boot's state
let later = new Map(); // islands waiting for hydrate(): instance -> its record
let woken = new Set(); // instances hydrate() was called for
let ready = Promise.resolve(); // the last start's boot

export function define(id, fn, opts) {
  defs.set(id, { fn, ...opts });
}

// ---- signals ----------------------------------------------------------------

let observer = null; // the node running: what it reads, it depends on
let queue = []; // nodes to run in the next batch
let flushing = null; // the next batch
let ids = 0; // creation order: a parent's nodes come before its children's
const all = new Set(); // DOM nodes: a start runs them again
const proxied = new WeakSet();
const proxies = new WeakMap(); // object -> its proxy

// Whether b written over a changes nothing. An object that is not a proxy
// (a Map, a Date) may have changed inside, so it is always news.
const same = (a, b) => Object.is(a, b) && (!a || typeof a != 'object' || proxied.has(a));

// A value that tells what read it when it changes. Deep: objects and arrays
// put in it are proxies, so a change inside them is a change too.
class Sig {
  constructor(x, deep) {
    this.d = deep;
    this.x = deep ? proxy(x) : x;
    this.subs = new Set();
  }
  get v() {
    track(this);
    return this.x;
  }
  set v(x) {
    if (this.d) x = proxy(x);
    if (same(x, this.x)) return;
    this.x = x;
    for (const n of this.subs) mark(n, 2);
  }
}

// A value worked out from others, lazily: again only when it is read after
// one of them changed, and what read it runs only if the result differs.
class Memo {
  constructor(f) {
    Object.assign(this, { f, st: 2, id: ++ids, deps: new Set(), subs: new Set(), stops: [], memo: 1 });
  }
  get v() {
    if (this.st) update(this);
    track(this);
    return this.x;
  }
}

function track(s) {
  const o = observer;
  if (o && !o.dead && !o.deps.has(s)) {
    o.deps.add(s);
    s.subs.add(o);
  }
}

// st: 0 current, 1 an input it derives from may have changed, 2 stale.
function mark(n, st) {
  if (n.st >= st) return;
  const was = n.st;
  n.st = st;
  if (was) return;
  if (n.memo) for (const m of n.subs) mark(m, 1);
  else queue.push(n), schedule();
}

const schedule = () => (flushing ||= Promise.resolve().then(flush));

function update(n) {
  if (n.st == 1) {
    for (const d of n.deps) {
      if (d.memo && d.st) update(d);
      if (n.st == 2) break;
    }
    if (n.st == 1) n.st = 0;
  }
  if (n.st == 2) exec(n);
}

// One failing node logs, once per error, and leaves the rest to run.
const failed = new WeakMap();
function exec(n) {
  n.st = 0;
  clean(n);
  const prev = observer;
  observer = n;
  try {
    const r = n.f();
    if (n.memo) {
      if (same(r, n.x)) return;
      n.x = r;
      for (const m of n.subs) if (m.st == 1) m.st = 2;
    } else if (n.u && typeof r == 'function') n.stops.push(r);
  } catch (e) {
    if (failed.get(n) !== String(e)) console.error(e);
    failed.set(n, String(e));
  } finally {
    observer = prev;
  }
}

function clean(n) {
  for (const d of n.deps) d.subs.delete(n);
  n.deps.clear();
  const s = n.stops;
  n.stops = [];
  run(s);
}

function dispose(n) {
  n.dead = 1;
  all.delete(n);
  clean(n);
}

// A node: f runs in the next batch, and again whenever what it read
// changes. An effect (fx) runs once the DOM has settled; a user's (u) may
// return its cleanup. It ends with its owner, a scope or an effect.
function node(owner, f, fx = 0, u = 0) {
  const n = { f, fx, u, id: ++ids, st: 0, deps: new Set(), stops: [] };
  owner?.stops.push(() => dispose(n));
  if (!u) all.add(n);
  mark(n, 2);
  return n;
}

export function untrack(f) {
  const prev = observer;
  observer = null;
  try {
    return f();
  } finally {
    observer = prev;
  }
}

// A batch: DOM nodes in creation order (what they make runs next round),
// then, once none are left, effects. Then what waited for its first batch.
function flush() {
  for (let round = 0; queue.length; round++) {
    const q = queue.some((n) => !n.fx) ? queue.filter((n) => !n.fx) : queue;
    queue = q === queue ? [] : queue.filter((n) => n.fx);
    if (round > 999) {
      console.error('wisp: state keeps changing as it is drawn (an effect that sets what it reads?)');
      for (const n of [...q, ...queue]) n.st = 0;
      queue = [];
      break;
    }
    q.sort((a, b) => a.id - b.id);
    for (const n of q) n.dead || update(n);
  }
  flushing = null;
  const now = mounts;
  mounts = [];
  for (const [inst, f] of now) {
    if (inst.sc.signal.aborted) continue;
    try {
      const r = f();
      if (typeof r == 'function') inst.sc.stops.push(r);
    } catch (e) {
      console.error(e);
    }
  }
}

export const tick = () => flushing || Promise.resolve();

// A plain object or array as a proxy whose every key is a signal: reads
// are tracked and writes tell only what read that key (or the keys, or the
// length). Anything else (a Date, an element) is left as it is.
function proxy(x) {
  if (!x || typeof x != 'object' || proxied.has(x) || Object.isFrozen(x)) return x;
  const proto = Object.getPrototypeOf(x);
  if (proto && proto != Object.prototype && proto != Array.prototype) return x;
  let p = proxies.get(x);
  if (p) return p;
  const sigs = new Map();
  const keys = new Sig(0);
  const bump = (s) => s && (s.v = s.x + 1);
  p = new Proxy(x, {
    get(t, k, r) {
      // Not methods: `list.map` reads the length and items it needs.
      if (observer && (Object.hasOwn(t, k) || !(k in t))) {
        if (!sigs.has(k)) sigs.set(k, new Sig(0));
        track(sigs.get(k));
      }
      return proxy(Reflect.get(t, k, r));
    },
    has(t, k) {
      track(keys);
      return k in t;
    },
    ownKeys(t) {
      track(keys);
      return Reflect.ownKeys(t);
    },
    set(t, k, v) {
      const had = Object.hasOwn(t, k);
      const old = t[k];
      const n = t.length;
      t[k] = v;
      if (!had) bump(keys);
      if (!had || !same(old, v)) bump(sigs.get(k));
      if (Array.isArray(t) && t.length !== n) {
        bump(sigs.get('length'));
        for (let i = t.length; i < n; i++) bump(sigs.get(String(i)));
      }
      return true;
    },
    deleteProperty(t, k) {
      if (Object.hasOwn(t, k)) {
        delete t[k];
        bump(sigs.get(k));
        bump(keys);
      }
      return true;
    },
  });
  proxied.add(p);
  proxies.set(x, p);
  return p;
}

// $state.snapshot: a plain copy, for structuredClone, a library or a log.
const snap = (x) =>
  !proxied.has(x) ? x : Array.isArray(x) ? x.map(snap) : Object.fromEntries(Object.entries(x).map(([k, v]) => [k, snap(v)]));

// ---- stores -----------------------------------------------------------------

// Calls f with get's value now, and again each time it changes.
function sub(get, f) {
  const n = node(null, () => {
    const v = get();
    untrack(() => f(v));
  }, 1, 1);
  update(n);
  return () => dispose(n);
}

// A value any code can share: `cart.value` reads it (tracked, like state),
// setting it or changing it in place updates what read it. Lives as long
// as the page, so across morphs and navigations; put one in a src/lib
// module to share it between files.
export function store(value) {
  const s = new Sig(value, 1);
  return {
    get value() {
      return s.v;
    },
    set value(v) {
      s.v = v;
    },
    set: (v) => (s.v = v),
    update: (f) => (s.v = f(s.x)),
    subscribe: (f) => sub(() => s.v, f),
  };
}

// A store kept in localStorage under `key`, and in step across tabs. It is
// saved whenever it changes, in place too. One per key: a script that asks
// again (each time it starts) gets the same.
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
  sub(() => JSON.stringify(s.value), (j) => {
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
  const m = new Memo(f);
  return { get value() { return m.v; }, subscribe: (g) => sub(() => m.v, g) };
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

function send(type, detail) {
  document.dispatchEvent(new CustomEvent(type, { detail }));
}

// Cleanups: one failing logs and leaves the rest to run.
function run(fns) {
  for (const f of fns) {
    try {
      f();
    } catch (e) {
      console.error(e);
    }
  }
}

// A node that calls f(value, first) when get's value changed, or when a
// start came between (a morph may have rewritten the DOM). `first` is true
// then too.
function watch(sc, get, L, f, again = true) {
  let last = NONE;
  let ep = epoch;
  node(sc, () => {
    const v = get(L);
    if (Object.is(v, last) && (ep === epoch || !again)) return;
    const first = last === NONE || ep !== epoch;
    ep = epoch;
    untrack(() => f((last = v), first));
  });
}

// A scope owns cleanups (its nodes' ends among them): one per instance,
// element and copy.
function scope() {
  const ac = new AbortController();
  return { stops: [() => ac.abort()], signal: ac.signal };
}

// ---- start ------------------------------------------------------------------

// Matches the instances in #wisp-live with the running ones, after the
// first load and after every morph. Islands, and what renders inside them,
// are left for hydrate(): their modules are not loaded yet.
function start() {
  const my = ++gen;
  const json = document.getElementById('wisp-live');
  const { m = {}, i = [], r = null, p = {} } = json ? JSON.parse(json.textContent) : {};
  route = { id: r, params: p };
  const at = {};
  const list = i.map(([I, id, P, ...x]) => {
    const rec = { I, id, P, blob: x.pop(), how: x[0] };
    rec.late = rec.how || at[P]?.late;
    return (at[I] = rec);
  });
  cur = { list, at, m };
  later = new Map();
  woken = new Set();
  const wait = need(list.filter((x) => !x.late), m);
  const go = () => my === gen && boot(list, my);
  // No await when all is loaded: restart before the next paint.
  ready = wait ? wait.then(go) : Promise.resolve(go());
}

// Imports the modules of these instances that are not in yet.
function need(list, m) {
  const want = [...new Set(list.map((x) => m[x.id]))].filter((u) => u && !done.has(u));
  return want.length && Promise.all(want.map((u) => import(u).catch((e) => console.error(e)).then(() => done.add(u))));
}

function boot(list, my) {
  epoch++;
  const els = {};
  for (const el of document.querySelectorAll('[data-w]')) {
    const [i, g] = el.dataset.w.split('.');
    if (g != null) (els[i] ||= []).push(el);
  }
  // waiting: instance -> what starts once it has (its +page.js loaded, or
  // it hydrated), so that getContext finds what its script sets.
  Object.assign(cur, { els, my, byI: {}, waiting: {}, kept: new Set(), old: live });
  live = [];
  list.forEach(begin);
  for (const o of cur.old) if (!cur.kept.has(o)) destroy(o);
  loaded = true;
  for (const n of all) mark(n, 2);
}

function begin(rec) {
  const { I, id, P, blob, how } = rec;
  const { els, byI, waiting, kept, old, my } = cur;
  const def = defs.get(id);
  const mine = els[I] || [];
  if (waiting[P]) return waiting[P].push(rec);
  // Kept: the instance whose element this is, or failing that (its
  // elements were all replaced) the one at the same place in the list.
  let inst = null;
  if (def && !mine.some((el) => el.closest('[data-wisp-reset]'))) {
    const ok = (o) => o && o.def === def && !kept.has(o) && old.includes(o) && !o.sc.signal.aborted;
    inst = mine.map((el) => recs.get(el)?.inst).find(ok) || old.find((o) => o.I === I && ok(o));
  }
  if (!inst && how && !woken.has(I)) {
    waiting[I] = [];
    later.set(I, rec);
    return;
  }
  if (!def) {
    // Inside an island that hydrated before: its module comes now.
    const w = need([rec], cur.m);
    if (w) w.then(() => my === gen && begin(rec));
    return;
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
}

// Starts island I (wisp.js calls this when it is seen, idle, ...): its
// module and those of what renders inside it load, then it binds. Resolves
// once it is drawn.
export function hydrate(I) {
  const g = gen;
  return ready.then(async () => {
    if (g !== gen) return;
    // One inside an island that waits still: it starts with that one.
    woken.add(I);
    const rec = later.get(I);
    if (!rec) return;
    later.delete(I);
    const { list, at, m, waiting } = cur;
    await need(list.filter((x) => { for (let y = x; y; y = at[y.P]) if (y === rec) return true; }), m);
    if (g !== gen) return;
    const next = waiting[I];
    delete waiting[I];
    begin(rec);
    next.forEach(begin);
    await tick();
  });
}

// Runs the module's +page.js `load` if it has one, then f with the values
// it gives as `data`. A failing load shows the error page.
function loadThen(def, blob, my, f) {
  if (!def.load) return f(blob);
  const url = new URL(location.href);
  const { id, params } = route;
  Promise.resolve()
    .then(() => def.load({ data: blob.data, url, params, route: { id }, fetch }))
    .then((data) => my === gen && f({ ...blob, data }), boundary);
}

// Runs a module's script for a new instance. A script that throws while
// starting shows the route's error page.
function create(def, id, blob, el, parent) {
  const inst = { def, id, I: -1, sc: scope(), recs: new Set(), ctx: new Map(), el, parent, g: [], events: {} };
  try {
    inst.g = untrack(() => def.fn(blob, helpers(inst))).g;
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
// instance. A timeout or frame that comes after teardown does nothing, so
// only what repeats has to be stopped.
function helpers(inst) {
  const sc = inst.sc;
  const signal = sc.signal;
  const wrap = (f) => (...a) => signal.aborted || f(...a);
  // $effect: after the DOM is drawn (pre: before), and again when what it
  // read changes. What it returns runs before that and at the end. One made
  // inside another ends with that run of it.
  const fx = (f, pre) => void node(observer?.u ? observer : sc, f, pre ? 0 : 1, 1);
  return {
    // What the compiler turns runes and state into.
    __wisp_s: (x) => new Sig(x, 1),
    __wisp_r: (x) => new Sig(x),
    __wisp_d(f) {
      const m = new Memo(f);
      sc.stops.push(() => dispose(m));
      return m;
    },
    __wisp_e: (f) => fx(f),
    __wisp_ep: (f) => fx(f, 1),
    __wisp_snap: snap,
    // The server values or props, as signals; `d` has $props() defaults.
    // New ones come in through inst.s: a morph's, or a parent's.
    __wisp_props(p, names, d = {}) {
      const val = (k, v) => (v === undefined && d[k] ? d[k]() : v);
      const P = {};
      for (const k of names) P[k] = new Sig(val(k, p[k]), 1);
      inst.P = P;
      inst.s = (n) => names.forEach((k) => (P[k].v = val(k, n[k])));
      return P;
    },
    untrack,
    tick,
    // effect(fn) is $effect(fn); effect(fn, () => [a, b]) runs when a or b change.
    effect: (f, deps) => fx(deps ? () => ((d) => untrack(() => f(d)))(deps()) : f),
    // watch(() => a, (a) => …) runs when a changes, not at the start.
    watch(get, f) {
      let last = NONE;
      fx(() => {
        const v = get();
        const was = last;
        last = v;
        if (was !== NONE && !Object.is(v, was)) return untrack(() => f(v));
      });
    },
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
    onMount: (f) => (mounts.push([inst, f]), schedule()),
    onDestroy: (f) => sc.stops.push(f),
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
        after = typeof f == 'function' && f({ form, formData, submitter, action, cancel: () => e.preventDefault() });
      });
      on('wisp:result', (e) => {
        page.value = { ...page.value, form: e.detail.data };
        Promise.resolve(after).then((g) => typeof g == 'function' && g(e.detail));
      });
      return () => ac.abort();
    },
  };
}

// ---- bindings -----------------------------------------------------------------

// Binds group g of inst to el with locals L. A quiet element does not
// animate in.
function setup(sc, inst, el, g, L, quiet) {
  for (const [kind, a, b, c, d] of inst.g[g] || []) {
    try {
      binding(sc, inst, el, L, quiet, kind, a, b, c, d);
    } catch (e) {
      console.error(e);
    }
  }
  if (!quiet && trans.has(el)) {
    let first = true;
    node(sc, () => {
      if (first && !el.hidden) play(el);
      first = false;
    });
  }
}

// An element of a copy (made here or painted by the server): bound to its
// group, with the Rust loop values it carries; or a component's slot,
// where what the page gave it goes.
function bindEl(el, sc, inst, L, quiet) {
  const w = el.dataset.w;
  if (w) return setup(sc, inst, el, +w, el.dataset.wl ? Object.assign(Object.create(L), JSON.parse(el.dataset.wl)) : L, quiet);
  const s = el.hasAttribute('data-wslot') && inst.slot;
  if (s) adopt(painted(el), sc, s.inst, s.L) || place(el, s.tpl, sc, s.inst, s.L, quiet);
}

function bindFrag(frag, sc, inst, L, quiet) {
  for (const el of frag.querySelectorAll('[data-w], template[data-wslot]')) bindEl(el, sc, inst, L, quiet);
}

// The server's copy right after `at`: its nodes from <!--[--> to the
// matching <!--]-->, or null.
function painted(at) {
  let n = at.nextSibling;
  if (!n || mark1(n) < 1) return null;
  const first = n;
  for (let d = 0; n && (d += mark1(n)); ) n = n.nextSibling;
  return { first, last: n };
}

// 1 at a painted copy's start, -1 at its end.
const mark1 = (n) => (n.nodeType == 8 ? (n.data == '[') - (n.data == ']') : 0);

// Takes a painted copy over: its nodes become ours, and its elements bind.
function adopt(c, sc, inst, L) {
  if (!c) return c;
  for (const n of range(c)) n.__w = 1;
  // Found first, then bound: binding puts nodes in (components, copies).
  const els = [];
  walk(c.first.nextSibling, c.last, els);
  for (const el of els) bindEl(el, sc, inst, L, true);
  return c;
}

// The elements from n to last and inside them, in order, but not those in
// copies painted inside (their own block takes those over).
function walk(n, last, out) {
  for (let d = 0; n && n !== last; n = n.nextSibling) {
    if (!d && n.nodeType == 1) {
      out.push(n);
      walk(n.firstChild, null, out);
    }
    d += mark1(n);
  }
}

// Removes the server's copies after `at`: a kept block's, after a morph.
function drop(at) {
  for (let c; (c = painted(at)); ) range(c).forEach((n) => n.remove());
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

const str = (v) => (v == null ? '' : String(v));

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
        if (!ms) return untrack(() => c(L, e));
        clearTimeout(timer);
        timer = setTimeout(() => signal.aborted || c(L, e), ms);
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
      el.addEventListener(a == 'checked' ? 'change' : 'input', () => c(L, read()), { signal });
      // A form reset (wisp.js resets a form its post succeeded with) moves
      // the field to its new default; the variable follows it.
      el.form?.addEventListener('reset', () => queueMicrotask(() => c(L, read())), { signal });
      // Compared with the element, not the last value: a handler may change
      // the target before the input's own batch.
      return void node(sc, () => {
        const v = a == 'checked' ? !!b(L) : b(L);
        if (read() === v) return;
        if (a == 'checked') el.checked = v;
        else el.value = v ?? '';
      });
    }
    case 'attr':
      return watch(sc, b, L, (v, first) => {
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
      return watch(sc, (L) => str(a(L)), L, (v) => (el.textContent = v));
    case 'hole': {
      // The anchor's value goes in one text node between it and the end
      // comment after it; a morph may have added or taken nodes there.
      let t = null;
      return watch(sc, (L) => str(a(L)), L, (s) => {
        if (!t || t.previousSibling !== el) {
          t = null;
          for (let n = el.nextSibling; n && n.nodeType != 8; ) {
            const next = n.nextSibling;
            if (!t && n.nodeType == 3) t = n;
            else n.remove();
            n = next;
          }
          if (!t) el.after((t = document.createTextNode('')));
        }
        if (t.data !== s) t.data = s;
      });
    }
    case 'class':
      return watch(sc, b, L, (v) => {
        const t = toggled.get(el) || new Map();
        toggled.set(el, t.set(a, !!v));
        el.classList.toggle(a, !!v);
      });
    case 'style':
      return watch(sc, b, L, (v) => {
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
      return watch(sc, b || (() => {}), L, (v, first) => (first ? (r = a(L)(el, v)) : r?.update?.(v)), false);
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
// into order. A kept copy's item and index are signals set in place, so
// only what reads them runs again. Copies' locals inherit the template's,
// so a nested block sees the outer item.
function clones(sc, inst, tpl, L, quiet, kind, get, names, keyOf) {
  let list = [];
  let first = true;
  sc.stops.push(() => list.forEach((c) => (run(c.sc.stops), range(c).forEach((n) => n.remove()))));
  const plain = (item, i) => {
    const o = Object.create(L);
    if (names[0]) o[names[0]] = item;
    if (names[1]) o[names[1]] = i;
    return o;
  };
  node(sc, () => {
    const items = kind == 'if' ? (get(L) ? [0] : []) : [...(get(L) ?? [])];
    const keys = items.map((item, i) => (keyOf ? keyOf(plain(item, i)) : i));
    untrack(() => {
      // The server's copies: the first draw takes them over in order; after
      // a morph that kept this block, the new page's go.
      const pre = [];
      if (!first) drop(list.at(-1)?.last || tpl);
      else for (let c, at = tpl; (c = painted(at)); at = c.last) pre.push(c);
      for (const c of pre.slice(items.length)) range(c).forEach((n) => n.remove());
      const old = new Map(list.map((c) => [c.key, c]));
      // animate:flip: where each element was, to slide it from there.
      const rects = new Map();
      for (const c of list) for (const n of range(c)) if (flips.has(n)) rects.set(n, n.getBoundingClientRect());
      const next = items.map((item, i) => {
        const key = keys[i];
        const c = old.get(key);
        if (c) {
          old.delete(key);
          c.s[0].v = item;
          c.s[1].v = i;
          return c;
        }
        const s = [new Sig(item), new Sig(i)];
        const cl = Object.create(L);
        names.forEach((n, k) => n && Object.defineProperty(cl, n, { get: () => s[k].v }));
        return { key, L: cl, s, sc: scope(), pre: pre[i] };
      });
      for (const c of old.values()) remove(sc, c);
      let at = tpl;
      for (const c of next) {
        if (!c.first) Object.assign(c, adopt(c.pre, c.sc, inst, c.L) || place(at, tpl, c.sc, inst, c.L, quiet && first));
        else if (at.nextSibling !== c.first) at.after(...range(c));
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
    });
  });
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
// module with the props from the page's code. Its script runs once; props
// that change go in as they do, and a `bind:` prop it changes comes back out.
function mount(sc, parent, anchor, L, quiet, id, props, binds, events) {
  const def = defs.get(id);
  // A component rendering itself (a tree) ends with its data; this stops
  // one that would not.
  const depth = (parent.depth || 0) + 1;
  if (depth > 64) return void console.error(`component ${id} nests more than 64 deep`);
  const child = { def, id, sc: scope(), recs: new Set(), ctx: new Map(), el: anchor, parent, g: [], events: {}, depth };
  child.slot = { tpl: anchor, inst: parent, L };
  for (const [name, f] of events) child.events[name] = (v) => untrack(() => f(L, v));
  const where = untrack(() => {
    try {
      child.g = def.fn({ ...props(L) }, helpers(child)).g;
    } catch (e) {
      console.error(e);
    }
    def.tpl ||= Object.assign(document.createElement('template'), { innerHTML: def.html || '' });
    return adopt(painted(anchor), child.sc, child, {}) || place(anchor, def.tpl, child.sc, child, {}, quiet);
  });
  sc.stops.push(() => (run(child.sc.stops), range(where).forEach((n) => n.remove())));
  node(sc, () => {
    const p = props(L);
    untrack(() => {
      drop(where.last); // after a morph, the new page's copy
      child.s?.(p);
    });
  });
  for (const [name, get, set] of binds)
    node(child.sc, () => {
      const v = child.P?.[name]?.v;
      untrack(() => Object.is(v, get(L)) || set(L, v));
    });
}

// ---- transitions ------------------------------------------------------------

// Plays el's transition in, or out (the same frames reversed); done runs
// when it finishes, not when a newer play on el cancels it.
function play(el, out, done) {
  anims.get(el)?.cancel();
  const t = trans.get(el);
  if (!t) return done?.();
  const o = { duration: 150, ...untrack(t[1]) };
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

// ---- navigation -------------------------------------------------------------

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
