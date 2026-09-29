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
// `count.v`), the objects, arrays, maps and sets in it are proxies that
// track each key, and every binding is a node that remembers what it read.
// A write queues only the nodes that read it. They run in one batch, in a
// microtask, parents before children and effects last; a derived value is
// worked out again only when it is read after one of its inputs changed.
// No virtual DOM, and a component's script runs once.
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
// module loaded. An element marked so waits here, its module already in.
//
// The less used half (transitions, {:#await}, bind:group, ...) is
// /_app/c/extra.js, which a module imports when it uses it: it adds its
// kinds of binding to `X` (exported as __wisp).

const defs = new Map(); // module id -> { fn, html, load }
const done = new Set(); // module urls imported
const seen = new WeakSet(); // elements bound before: a restart does not animate them in
const recs = new WeakMap(); // element -> its binding record { inst, key, sc, el }
const FLAGS = 'prevent stop once self capture passive window document outside debounce ctrl shift alt meta'.split(' ');
const KEYS = { space: ' ', up: 'arrowup', down: 'arrowdown', left: 'arrowleft', right: 'arrowright' };
// Events that bubble, handled by one listener on the root for the page.
const DELEGATE = new Set('click dblclick input change keydown keyup pointerdown pointerup pointermove mousedown mouseup contextmenu focusin focusout'.split(' '));
const RAW = Symbol(); // proxy[RAW]: the object itself, tracking any change in it
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
let current = null; // the instance whose script is running, for context()

export function define(id, fn, opts) {
  defs.set(id, { fn, ...opts });
}

// ---- signals ----------------------------------------------------------------

let observer = null; // the node running: what it reads, it depends on
let queue = []; // nodes to run in the next batch
let flushing = null; // the next batch
let ids = 0; // creation order: a parent's nodes come before its children's
let runs = 0; // per node run: a signal read twice in one is tracked once
const all = new Set(); // DOM nodes: a start runs them again
const proxied = new WeakSet();
const proxies = new WeakMap(); // object -> its proxy

// Whether b written over a changes nothing. An object that is not a proxy
// (a Date, an element) may have changed inside, so it is always news.
const same = (a, b) => Object.is(a, b) && (!a || typeof a != 'object' || proxied.has(a));

// A value that tells what read it when it changes. Deep: objects and arrays
// put in it are proxies, so a change inside them is a change too.
class Sig {
  constructor(x, deep) {
    this.d = deep;
    this.x = deep ? proxy(x) : x;
    this.subs = null;
    this.t = 0;
  }
  get v() {
    track(this);
    return this.x;
  }
  set v(x) {
    if (this.d) x = proxy(x);
    if (same(x, this.x)) return;
    this.x = x;
    if (this.subs) for (const n of this.subs) mark(n, 2);
  }
}

// A value worked out from others, lazily: again only when it is read after
// one of them changed, and what read it runs only if the result differs.
class Memo {
  constructor(f, sc) {
    Object.assign(this, { f, sc, st: 2, id: ++ids, deps: [], subs: null, stops: null, memo: 1, t: 0, run: 0 });
  }
  get v() {
    if (this.st) update(this);
    track(this);
    return this.x;
  }
}

function track(s) {
  const o = observer;
  if (o && !o.dead && s.t !== o.run) {
    s.t = o.run;
    o.deps.push(s);
    (s.subs ||= new Set()).add(o);
  }
}

// st: 0 current, 1 an input it derives from may have changed, 2 stale.
function mark(n, st) {
  if (n.st >= st) return;
  const was = n.st;
  n.st = st;
  if (was) return;
  if (!n.memo) queue.push(n), schedule();
  else if (n.subs) for (const m of n.subs) mark(m, 1);
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

// A failing node goes to its {:#try} block, if it is in one; else it logs,
// once per error, and leaves the rest to run.
const failed = new WeakMap();
function exec(n) {
  n.st = 0;
  clean(n);
  const prev = observer;
  observer = n;
  n.run = ++runs;
  try {
    const r = n.f();
    if (n.memo) {
      if (same(r, n.x)) return;
      n.x = r;
      if (n.subs) for (const m of n.subs) if (m.st == 1) m.st = 2;
    } else if (n.u && typeof r == 'function') (n.stops ||= []).push(r);
  } catch (e) {
    const b = n.sc?.b;
    if (b) b(e);
    else if (failed.get(n) !== String(e)) console.error(e), failed.set(n, String(e));
  } finally {
    observer = prev;
  }
}

function clean(n) {
  for (const d of n.deps) d.subs?.delete(n);
  n.deps.length = 0;
  const s = n.stops;
  if (s) (n.stops = null), run(s);
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
  const n = { f, fx, u, sc: owner?.sc || owner, id: ++ids, st: 0, deps: [], stops: null, run: 0, dead: 0 };
  if (owner) (owner.stops ||= []).push(n);
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
    let q = [];
    const fx = [];
    for (const n of queue) (n.fx ? fx : q).push(n);
    if (q.length) queue = fx;
    else (q = fx), (queue = []);
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
    if (inst.sc.dead) continue;
    try {
      const r = f();
      if (typeof r == 'function') inst.sc.stops.push(r);
    } catch (e) {
      console.error(e);
    }
  }
}

export const tick = () => flushing || Promise.resolve();

// A plain object or array, or a Map or a Set, as a proxy whose every key is a
// signal: reads are tracked, and writes tell only what read that key (or
// the keys, or the length). proxy[RAW] tracks any change. Anything else (a
// Date, an element, a class instance) is left as it is.
function proxy(x) {
  if (!x || typeof x != 'object' || proxied.has(x) || Object.isFrozen(x)) return x;
  let p = proxies.get(x);
  if (p) return p;
  const proto = Object.getPrototypeOf(x);
  // A Map or a Set is state once extra.js is in (a script that makes one
  // imports it).
  const coll = x instanceof Map || x instanceof Set;
  if (coll ? !X.coll : proto && proto != Object.prototype && proto != Array.prototype) return x;
  const sigs = new Map();
  const keys = new Sig(0); // the keys, for `in`, iteration and size
  const ver = new Sig(0); // any change
  const sig = (k) => sigs.get(k) || (sigs.set(k, new Sig(0)), sigs.get(k));
  const bump = (s) => s && (s.v = s.x + 1);
  const changed = (k, added) => {
    bump(sigs.get(k));
    if (added) bump(keys);
    bump(ver);
  };
  p = new Proxy(
    x,
    coll
      ? X.coll({ sig, keys, ver, sigs, bump, changed }, () => p)
      : {
          get(t, k, r) {
            if (k === RAW) return track(ver), t;
            // Not methods: `list.map` reads the length and items it needs.
            if (observer && (Object.hasOwn(t, k) || !(k in t))) track(sig(k));
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
            if (!had || !same(old, v)) changed(k, !had);
            if (Array.isArray(t) && t.length !== n) {
              bump(sigs.get('length'));
              for (let i = t.length; i < n; i++) bump(sigs.get(String(i)));
            }
            return true;
          },
          deleteProperty(t, k) {
            if (Object.hasOwn(t, k)) delete t[k], changed(k, 1);
            return true;
          },
        },
  );
  proxied.add(p);
  proxies.set(x, p);
  return p;
}

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

// A context of its own: `const [getUser, setUser] = context()`, in a lib
// module or a script. Set in a component's script, got in its own or a
// descendant's, as they start.
export function context() {
  const k = Symbol();
  return [() => find(current, k), (v) => (current?.ctx.set(k, v), v)];
}

function find(inst, k) {
  for (let i = inst; i; i = i.parent) if (i.ctx.has(k)) return i.ctx.get(k);
}

// use:portal="'#modal'" (or bare, for <body>): the element lives there
// instead, and goes with the code that made it.
export function portal(el, to) {
  (typeof to == 'string' ? document.querySelector(to) : to || document.body).append(el);
  el.__w = 1;
  return () => el.remove();
}

function send(type, detail) {
  document.dispatchEvent(new CustomEvent(type, { detail }));
}

// Cleanups (functions, or nodes to end): one failing logs and leaves the
// rest to run.
function run(fns) {
  for (const f of fns) {
    try {
      typeof f == 'function' ? f() : dispose(f);
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
  const n = node(sc, () => {
    const v = get(L);
    if (Object.is(v, last) && (ep === epoch || !again)) return;
    const first = last === NONE || ep !== epoch;
    ep = epoch;
    last = v;
    observer = null; // untracked, as untrack() does, without its closure
    try {
      f(v, first);
    } finally {
      observer = n;
    }
  });
}

// A scope owns cleanups (its nodes among them): one per instance, element
// and copy. `b` is the {:#try} block it is in.
const scope = (up) => ({ stops: [], b: up?.b, dead: 0 });

function end(sc) {
  if (!sc.dead) (sc.dead = 1), run(sc.stops);
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
    const ok = (o) => o && o.def === def && !kept.has(o) && old.includes(o) && !o.sc.dead;
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

// An instance of a module: its script runs once, now.
function instance(def, id, el, parent, depth) {
  return { def, id, I: -1, sc: scope(), recs: new Set(), ctx: new Map(), el, parent, g: [], events: {}, depth };
}

function script(inst, blob) {
  const prev = current;
  current = inst;
  try {
    inst.g = untrack(() => inst.def.fn(blob, helpers(inst))).g;
  } finally {
    current = prev;
  }
}

// A script that throws while starting shows the route's error page.
function create(def, id, blob, el, parent) {
  const inst = instance(def, id, el, parent, 0);
  try {
    script(inst, blob);
  } catch (e) {
    boundary(e);
  }
  return inst;
}

function destroy(inst) {
  for (const r of inst.recs) stopRec(r);
  end(inst.sc);
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
      const quiet = !loaded || seen.has(el);
      const go = () => setup(r.sc, inst, el, +el.dataset.w.split('.')[1], JSON.parse(el.dataset.wl || '{}'), quiet);
      const w = waits(el);
      w ? w.then(() => r.sc.dead || go()) : go();
      seen.add(el);
    }
    keep.add(r);
  }
  for (const r of inst.recs) if (!keep.has(r)) stopRec(r);
}

function stopRec(r) {
  end(r.sc);
  r.inst.recs.delete(r);
  if (recs.get(r.el) === r) recs.delete(r.el);
}

// The wait of the closest element around el marked client:visible (or
// idle, ...) that has not come yet, or none.
function waits(el) {
  if (X.waiting) for (let n = el.parentNode; n; n = n.parentNode) if (n.__wait) return n.__wait;
}

// Asks wisp.js for the route's error page, once per navigation.
function boundary(e) {
  console.error(e);
  if (erred) return;
  erred = true;
  send('wisp:error', { error: e });
}

// What every module's function gets, whatever the instance.
const shared = {
  __wisp_s: (x) => new Sig(x, 1),
  __wisp_r: (x) => new Sig(x),
  untrack,
  tick,
  derived,
  store,
  persisted,
  goto,
  invalidate,
  page,
  navigating,
  context,
  portal,
};

// What a module's function gets. Timers and listeners end with the
// instance. A timeout or frame that comes after teardown does nothing, so
// only what repeats has to be stopped.
function helpers(inst) {
  const sc = inst.sc;
  const wrap = (f) => (...a) => sc.dead || f(...a);
  const signal = () => {
    if (!inst.ac) (inst.ac = new AbortController()), sc.stops.push(() => inst.ac.abort());
    return inst.ac.signal;
  };
  // $effect: after the DOM is drawn (pre: before), and again when what it
  // read changes. What it returns runs before that and at the end. One made
  // inside another ends with that run of it.
  const fx = (f, pre) => void node(observer?.u ? observer : sc, f, pre ? 0 : 1, 1);
  return {
    __proto__: shared,
    __wisp_d(f) {
      const m = new Memo(f, sc);
      sc.stops.push(m);
      return m;
    },
    __wisp_e: (f) => fx(f),
    __wisp_ep: (f) => fx(f, 1),
    // The server values or props, as signals; `d` has $props() defaults
    // (for a prop not given, or null), and `rest` asks for `__rest`, an
    // object of the props not named. New ones come in through inst.s: a
    // morph's, or a parent's.
    __wisp_props(p, names, d = {}, rest) {
      const val = (k, v) => (v == null && d[k] ? d[k]() : v);
      const others = (n) => {
        const o = Object.fromEntries(n.__rest || []);
        for (const k in n) if (!names.includes(k) && k != '__rest') o[k] = n[k];
        return o;
      };
      const P = {};
      for (const k of names) P[k] = new Sig(val(k, p[k]), 1);
      if (rest) P.__rest = new Sig(others(p), 1);
      inst.P = P;
      inst.s = (n) => {
        for (const k of names) P[k].v = val(k, n[k]);
        if (rest) P.__rest.v = others(n);
      };
      return P;
    },
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
    addEventListener: (type, f, options) => addEventListener(type, wrap(f), { ...options, signal: signal() }),
    listen(url, f) {
      const es = new EventSource(url);
      es.onmessage = (e) => wrap(f)(e.data, e);
      sc.stops.push(() => es.close());
      return es;
    },
    onMount: (f) => (mounts.push([inst, f]), schedule()),
    onDestroy: (f) => sc.stops.push(f),
    emit: (name, value) => inst.events[name]?.(value),
    setContext: (k, v) => (inst.ctx.set(k, v), v),
    getContext: (k) => find(inst, k),
  };
}

// ---- bindings -----------------------------------------------------------------

// Binds group g of inst to el with locals L. A quiet element does not
// animate in. The group may start with where its directives go (`at`:
// <wisp:window> and the like), when it starts (`wait`), or its tag (`tag`:
// <wisp:element this={:…}>), which takes the rest.
function setup(sc, inst, el, g, L, quiet) {
  const bs = inst.g[g] || [];
  let k = 0;
  const [kind, a] = bs[0] || [];
  if (kind == 'at') (el = a == 'window' ? window : a == 'document' ? document : document.body), k++;
  else if (kind == 'wait') {
    // What is inside waits too (see waits()); once it came, never again.
    if (!el.__woke) return X.wait(el, a, () => sc.dead || setup(sc, inst, el, g, L, quiet));
    k++;
  } else if (kind == 'tag') return X.tag(sc, inst, el, bs, L, quiet);
  for (; k < bs.length; k++) {
    try {
      binding(sc, inst, el, L, quiet, bs[k]);
    } catch (e) {
      if (sc.b) sc.b(e);
      else console.error(e);
    }
  }
  if (!quiet) X.enter?.(sc, el);
}

// An element of a copy (made here or painted by the server): bound to its
// group, with the Rust loop values it carries; or a component's slot,
// where what the page gave it goes.
function bindEl(el, sc, inst, L, quiet, w, wl) {
  const wt = waits(el);
  if (wt) return void wt.then(() => sc.dead || bindEl(el, sc, inst, L, quiet, w, wl));
  if (w) return setup(sc, inst, el, +w, wl ? Object.assign(Object.create(L), wl) : L, quiet);
  const s = el.nodeType == 1 && el.hasAttribute('data-wslot') && inst.slot;
  if (s) adopt(painted(el), sc, s.inst, s.L) || place(el, s.tpl, sc, s.inst, s.L, quiet);
}

// Where tpl's content has nodes to bind, as child index paths, found once
// per template: [path, group, loop values]. The content is made lighter
// first: a {:hole}'s anchor and end become one text node, which the hole
// writes, and whitespace between table rows and cells goes.
const TABLE = /^(TABLE|THEAD|TBODY|TFOOT|TR|COLGROUP)$/;
function paths(tpl) {
  if (tpl.__p) return tpl.__p;
  const trim = (n) => {
    for (let c = n.firstChild, next; c; c = next) {
      next = c.nextSibling;
      if (c.nodeType == 3 && TABLE.test(n.nodeName) && !c.data.trim()) c.remove();
      else if (c.nodeType == 1) trim(c);
    }
  };
  trim(tpl.content);
  const out = [];
  const walk = (n, path) => {
    let i = 0;
    for (let c = n.firstChild; c; c = c.nextSibling, i++) {
      if (c.nodeType != 1) continue;
      const p = [...path, i];
      const [w, wl] = [c.dataset.w, c.dataset.wl && JSON.parse(c.dataset.wl)];
      const end = c.nextSibling;
      if (c.localName == 'template' && w && !c.content.firstChild && end?.nodeType == 8 && !end.data) {
        const t = document.createTextNode('');
        end.remove();
        c.replaceWith(t);
        out.push([p, w, wl]);
        c = t;
      } else {
        if (w || c.hasAttribute('data-wslot')) out.push([p, w, wl]);
        walk(c, p);
      }
    }
  };
  walk(tpl.content, []);
  // An end marker only where what comes last could grow after it.
  const last = tpl.content.lastChild;
  tpl.__end = !last || last.localName == 'template';
  return (tpl.__p = out);
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
  const [o, f] = [X.outs, X.flips];
  for (const el of els) bindEl(el, sc, inst, L, true, el.dataset.w, el.dataset.wl && JSON.parse(el.dataset.wl));
  return Object.assign(c, { t: X.outs != o, f: X.flips != f });
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

// A copy of tpl's content, bound to inst: a fragment, its first node and
// its last (an end marker, so what is later put inside the range moves and
// goes with it), and whether it has elements that play out (t) or slide
// (f).
function copy(tpl, sc, inst, L, quiet) {
  const ps = paths(tpl);
  const frag = tpl.content.cloneNode(true);
  const found = [];
  for (const [p, w, wl] of ps) {
    let n = frag;
    for (const i of p) {
      n = n.firstChild;
      for (let k = 0; k < i; k++) n = n.nextSibling;
    }
    found.push([n, w, wl]);
  }
  if (tpl.__end) frag.append(document.createComment(''));
  for (let n = frag.firstChild; n; n = n.nextSibling) n.__w = 1;
  const [o, f] = [X.outs, X.flips];
  for (const [el, w, wl] of found) bindEl(el, sc, inst, L, quiet, w, wl);
  return { first: frag.firstChild, last: frag.lastChild, frag, t: X.outs != o, f: X.flips != f };
}

function place(at, tpl, sc, inst, L, quiet) {
  const c = copy(tpl, sc, inst, L, quiet);
  at.after(c.frag);
  return c;
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

// `class={:[a, { b: on }]}`: the names of strings, and the keys whose
// value holds, in arrays and objects, at any depth.
const cls = (v) =>
  !v || typeof v != 'object' ? str(v === true ? '' : v || '') : Array.isArray(v) ? v.map(cls).filter(Boolean).join(' ') : Object.keys(v).filter((k) => v[k]).join(' ');

// `style={:{ color: c, fontSize: '2em' }}`: set properties; null and false
// ones left out.
const css = (v) =>
  Object.entries(v)
    .filter(([, x]) => x != null && x !== false)
    .map(([k, x]) => `${k.startsWith('--') ? k : k.replace(/[A-Z]/g, (m) => '-' + m.toLowerCase())}:${x}`)
    .join(';');

// The modifiers of an on: directive, worked out once per directive.
function mods(b, type) {
  const has = (m) => b.includes(m);
  const t = has('debounce') && b[b.indexOf('debounce') + 1];
  return {
    has,
    keys: b.filter((m) => !FLAGS.includes(m) && !/^\d+m?s$/.test(m)).map((k) => KEYS[k] || k),
    held: ['ctrl', 'shift', 'alt', 'meta'].filter(has),
    ms: !has('debounce') ? 0 : /s$/.test(t || '') ? parseInt(t) * (/ms$/.test(t) ? 1 : 1000) : 250,
    // Handled at the root, unless it has to be where it is.
    root: DELEGATE.has(type) && !['capture', 'passive', 'window', 'document', 'outside'].some(has),
  };
}

// The root's listener for delegated events: the handlers of each element
// from the target up, as if on each, until one stops the event.
const rooted = new Set();
function delegate(e) {
  for (let n = e.target; n; n = n.parentNode) {
    const hs = n.__on;
    if (!hs) continue;
    for (const h of hs) {
      if (h[0] != e.type || h[2].dead) continue;
      Object.defineProperty(e, 'currentTarget', { configurable: true, value: n });
      h[1](e);
    }
    if (e.cancelBubble) break;
  }
}

function binding(sc, inst, el, L, quiet, bnd) {
  const [kind, a, b, c] = bnd;
  switch (kind) {
    case 'on': {
      const m = (bnd.m ||= mods(b, a));
      const at = m.has('window') ? window : m.has('document') || m.has('outside') ? document : el;
      let timer, spent;
      const h = (e) => {
        if (m.has('self') && e.target !== el) return;
        if (m.has('outside') && e.composedPath().includes(el)) return; // the path survives the target's removal
        if (m.keys.length && !m.keys.includes(e.key?.toLowerCase())) return;
        if (m.held.some((k) => !e[k + 'Key'])) return;
        // .once by hand, so an event the filters above turn away does not use it up.
        if (spent) return;
        spent = m.has('once');
        if (m.has('prevent')) e.preventDefault();
        if (m.has('stop')) e.stopPropagation();
        if (!m.ms) return untrack(() => c(L, e));
        clearTimeout(timer);
        timer = setTimeout(() => sc.dead || c(L, e), m.ms);
      };
      if (m.root && at === el && el.nodeType == 1) {
        if (!rooted.has(a)) rooted.add(a), document.documentElement.addEventListener(a, delegate);
        // A kept element bound again drops its old handlers.
        el.__on = el.__on ? el.__on.filter((x) => !x[2].dead) : [];
        el.__on.push([a, h, sc]);
        return;
      }
      const o = { capture: m.has('capture'), passive: m.has('passive') };
      at.addEventListener(a, h, o);
      return void sc.stops.push(() => at.removeEventListener(a, h, o));
    }
    case 'bind':
      return a == 'this' ? c(L, el) : a == 'value' || a == 'checked' ? bind(sc, el, L, a, b, c) : X.bind(sc, el, L, a, b, c);
    case 'attr':
      return watch(sc, b, L, (v, first) => {
        // aria-* states are "true" or "false"; elsewhere false means absent.
        const aria = a.startsWith('aria-');
        if (a == 'class' && v && typeof v == 'object') v = cls(v);
        else if (a == 'style' && v && typeof v == 'object') v = css(v);
        const s = v == null || (v === false && !aria) ? null : v === true && !aria ? '' : String(v);
        const put = () => {
          if (s == null) el.removeAttribute(a);
          else el.setAttribute(a, s);
          if (a == 'value') el.value = s ?? '';
          else if (a == 'checked' || a == 'selected') el[a] = s != null;
          else if (a == 'class') el.__cls?.forEach((on, name) => el.classList.toggle(name, on));
        };
        // A transition plays as `hidden` turns off, and before it turns on.
        if (a != 'hidden' || first || !X.hide) return put();
        X.hide(el, s != null, put, sc);
      });
    case 'text':
      return watch(sc, (L) => str(a(L)), L, (v) => (el.textContent = v));
    case 'hole': {
      // The anchor's value goes in one text node between it and the end
      // comment after it; a morph may have added or taken nodes there.
      // In a copy, the anchor is the text node itself (see paths()).
      if (el.nodeType == 3) return watch(sc, (L) => str(a(L)), L, (s) => (el.data = s));
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
        (el.__cls ||= new Map()).set(a, !!v);
        el.classList.toggle(a, !!v);
      });
    case 'style':
      return watch(sc, b, L, (v) => {
        if (v == null || v === false) el.style.removeProperty(a);
        else el.style.setProperty(a, v);
      });
    case 'use': {
      let r;
      sc.stops.push(() => (typeof r == 'function' ? r() : r?.destroy?.()));
      return watch(sc, b || (() => {}), L, (v, first) => (first ? (r = a(L)(el, v)) : r?.update?.(v)), false);
    }
    case 'each':
    case 'if':
    case 'key':
      return clones(sc, inst, el, L, quiet, kind, a, b || [], c);
    default:
      return X[kind](sc, inst, el, L, quiet, bnd);
  }
}

// bind:value and bind:checked: the element's property and a script
// variable, both ways. What the user typed before this ran wins, then the
// script's value, then what the page shows.
function bind(sc, el, L, a, get, set) {
  const on = (at, type, f) => (at.addEventListener(type, f), sc.stops.push(() => at.removeEventListener(type, f)));
  const multi = el.multiple && el.localName == 'select';
  const read = () =>
    a == 'checked' ? el.checked : multi ? [...el.selectedOptions].map((o) => o.value) : /^(number|range)$/.test(el.type) && el.value !== '' ? +el.value : el.value;
  const typed = a == 'checked' ? el.checked !== el.defaultChecked : 'defaultValue' in el && el.value !== el.defaultValue;
  if (typed || get(L) == null) set(L, read());
  on(el, a == 'checked' || el.localName == 'select' ? 'change' : 'input', () => set(L, read()));
  // A form reset (wisp.js resets a form its post succeeded with) moves
  // the field to its new default; the variable follows it.
  if (el.form) on(el.form, 'reset', () => queueMicrotask(() => set(L, read())));
  // Compared with the element, not the last value: a handler may change
  // the target before the input's own batch.
  node(sc, () => {
    const v = get(L);
    untrack(() => {
      if (multi) for (const o of el.options) o.selected = !!v?.includes?.(o.value);
      else if (a == 'checked') el.checked = !!v;
      else if (read() !== v) el.value = v ?? '';
    });
  });
}

// {:#each}, {:#if}, {:#key} (and <template each|if>): one copy of the
// template's content per item, after it, kept by key (by index without
// one; {:#key}'s value is its one key) and moved into order. A kept copy's
// item and index are signals set in place, so only what reads them runs
// again. Copies' locals inherit the template's, so a nested block sees the
// outer item.
function clones(sc, inst, tpl, L, quiet, kind, get, names, keyOf) {
  let list = [];
  let first = true;
  sc.stops.push(() => {
    for (const c of list) end(c.sc);
    cut(list);
  });
  // What each copy's locals inherit: the item and index, read from the
  // signals the copy keeps under S.
  const S = Symbol();
  const P = Object.create(L);
  names.forEach((n, k) => n && Object.defineProperty(P, n, { get() { return this[S][k].v; } }));
  const tmp = Object.create(L); // a key's locals
  node(sc, () => {
    const v = get(L);
    // A state array is read whole, as one signal, not item by item.
    const raw = v?.[RAW];
    const items = kind == 'if' ? (v ? [0] : []) : kind == 'key' ? [v] : typeof v == 'number' ? Array.from({ length: v }, (_, i) => i) : raw ? raw.slice() : [...(v ?? [])];
    if (raw) for (let i = 0; i < items.length; i++) items[i] = proxy(items[i]);
    untrack(() => {
      const keys = kind == 'key' ? items : keyOf ? items.map((item, i) => (names[0] && (tmp[names[0]] = item), names[1] && (tmp[names[1]] = i), keyOf(tmp))) : null;
      // The server's copies: the first draw takes them over in order; after
      // a morph that kept this block, the new page's go.
      const pre = [];
      if (!first) drop(list.at(-1)?.last || tpl);
      else for (let c, at = tpl; (c = painted(at)); at = c.last) pre.push(c);
      for (const c of pre.splice(items.length)) range(c).forEach((n) => n.remove());
      // Where the block ends, found before any copy leaves.
      const tail = (list.at(-1)?.last || pre.at(-1)?.last || tpl).nextSibling;
      const old = new Map();
      list.forEach((c, i) => ((c.i = i), old.set(c.key, c)));
      const next = items.map((item, i) => {
        const key = keys ? keys[i] : i;
        const c = old.get(key);
        if (c) {
          old.delete(key);
          c.$[0].v = item;
          if (c.$[1]) c.$[1].v = i;
          return c;
        }
        const cl = Object.create(P);
        const n = { key, L: cl, $: (cl[S] = [new Sig(item), names[1] && new Sig(i)]), sc: scope(sc), i: -1 };
        // A painted copy is taken over where it is.
        if (pre[i]) Object.assign(n, adopt(pre[i], n.sc, inst, cl), { i });
        return n;
      });
      // Leaving: all at once when nothing plays out.
      const gone = [...old.values()];
      for (const c of gone) end(c.sc);
      const quick = gone.filter((c) => !c.t);
      if (quick.length == list.length) cut(quick);
      else quick.forEach((c) => range(c).forEach((n) => n.remove()));
      for (const c of gone) if (c.t) X.leave(sc, c);
      const flip = list.some((c) => c.f) && X.flip(list, old);
      order(tpl.parentNode, tail, next, (c) => Object.assign(c, copy(tpl, c.sc, inst, c.L, quiet && first)));
      list = next;
      first = false;
      if (flip) flip();
    });
  });
}

// Removes copies that are all of a block's, from the first's first node to
// the last's last node.
function cut(cs) {
  if (!cs.length) return;
  const r = document.createRange();
  r.setStartBefore(cs[0].first);
  r.setEndAfter(cs.at(-1).last);
  r.deleteContents();
}

// Puts the copies `next` in order before `tail`: old ones (i: where they
// were) and new ones (i < 0), which `make` draws. The old ones in the
// longest run already in order stay; the others move, and new ones go in
// together, a fragment per run of them.
function order(parent, tail, next, make) {
  const stay = lis(next.map((c) => c.i));
  let at = tail;
  let run = null;
  let runAt = null;
  const put = () => run && (parent.insertBefore(run, runAt), (run = null));
  for (let k = next.length - 1; k >= 0; k--) {
    const c = next[k];
    if (c.i < 0) {
      make(c);
      if (!run) (run = document.createDocumentFragment()), (runAt = at);
      run.insertBefore(c.frag, run.firstChild);
      c.frag = null;
    } else {
      put();
      if (!stay[k]) {
        const f = document.createDocumentFragment();
        f.append(...range(c));
        parent.insertBefore(f, at);
      }
    }
    at = c.first;
  }
  put();
}

// The indexes (in `a`) of a longest increasing run of old positions, as
// flags: those copies need not move. -1 (new) is never in it.
function lis(a) {
  const tails = [];
  const prev = new Array(a.length);
  for (let i = 0; i < a.length; i++) {
    const x = a[i];
    if (x < 0) continue;
    let lo = 0;
    let hi = tails.length;
    while (lo < hi) {
      const mid = (lo + hi) >> 1;
      if (a[tails[mid]] < x) lo = mid + 1;
      else hi = mid;
    }
    prev[i] = lo ? tails[lo - 1] : -1;
    tails[lo] = i;
  }
  const keep = new Array(a.length).fill(false);
  for (let i = tails.at(-1) ?? -1; i >= 0; i = prev[i]) keep[i] = true;
  return keep;
}

// ---- extensions -------------------------------------------------------------

// What extra.js builds on, and where it puts its kinds of binding and its
// hooks. `outs` and `flips` count the elements given an out transition and
// animate:flip, so a copy knows whether it has any; `waiting` the elements
// whose client:* has not come.
const X = {
  ...{ Sig, node, watch, scope, end, untrack, clones, binding, range, cls, css, track, proxy, same, proxied, shared },
  ...{ defs, instance, script, adopt, painted, place, drop, RAW },
  outs: 0,
  flips: 0,
  waiting: 0,
};
export const __wisp = X;

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
