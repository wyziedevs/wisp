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
const recs = new WeakMap(); // element -> its binding record { inst, w, wl, sc, el }
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
const all = new Set(); // DOM nodes outside copies: a start runs them again
const proxied = new WeakSet();

// Whether b written over a changes nothing. An object that is not a proxy
// (a Date, an element) may have changed inside, so it is always news.
const same = (a, b) => Object.is(a, b) && (!a || typeof a != 'object' || proxied.has(a));

// Links run both ways in flat arrays: a node's deps are [signal, where the
// node is in its subs, ...] and a signal's subs [node, where the signal is
// in its deps, ...], so either end unlinks in constant time.
const notify = (s, st) => {
  if (s) for (let i = 0; i < s.length; i += 2) mark(s[i], st);
};

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
    const q = this.eq;
    if (q) bump(q.get(this.x)), bump(q.get(x));
    this.x = x;
    notify(this.subs, 2);
  }
}

// `x === v` in markup, x a state variable: what reads it runs again only
// when x becomes v or stops being it, not on every change of x (a list's
// `selected === row.id` redraws two rows, not all). A signal per v read,
// dropped with its last reader (see unlink()).
function eq(s, v) {
  if (observer) {
    const q = (s.eq ||= new Map());
    let p = q.get(v);
    if (!p) q.set(v, (p = new Sig(0))), (p.of = q), (p.k = v);
    track(p);
  }
  return s.x === v;
}

// A value worked out from others, lazily: again only when it is read after
// one of them changed, and what read it runs only if the result differs.
class Memo {
  constructor(f, sc) {
    Object.assign(this, { f, sc, st: 2, id: ++ids, deps: null, di: 0, subs: null, stops: null, memo: 1, t: 0, run: 0 });
  }
  get v() {
    if (this.st) update(this);
    track(this);
    return this.x;
  }
}

// A run reads mostly what the last one did: a read that matches the next
// dependency just steps over it (di), and only from the first that differs
// are links undone and made again.
function track(s) {
  const o = observer;
  if (!o || o.dead || s.t === o.run) return;
  s.t = o.run;
  const d = (o.deps ||= []);
  if (d[o.di] === s) return void (o.di += 2);
  if (o.di < d.length) unlink(o, o.di);
  const subs = (s.subs ||= []);
  d.push(s, subs.length);
  subs.push(o, o.di);
  o.di += 2;
}

// Drops n's dependencies from index i on.
function unlink(n, i) {
  const d = n.deps;
  for (let k = d.length - 2; k >= i; k -= 2) {
    const s = d[k];
    const subs = s.subs;
    const slot = d[k + 1];
    const at = subs.pop();
    const o = subs.pop();
    if (slot < subs.length) {
      subs[slot] = o;
      subs[slot + 1] = at;
      o.deps[at + 1] = slot;
    } else if (!slot && s.of) s.of.delete(s.k);
  }
  d.length = i;
}

// st: 0 current, 1 an input it derives from may have changed, 2 stale.
function mark(n, st) {
  if (n.st >= st) return;
  const was = n.st;
  n.st = st;
  if (was) return;
  if (!n.memo) queue.push(n), schedule();
  else notify(n.subs, 1);
}

const schedule = () => (flushing ||= Promise.resolve().then(flush));

function update(n) {
  const d = n.deps;
  if (n.st == 1 && d) {
    for (let i = 0; i < d.length && n.st == 1; i += 2) if (d[i].memo && d[i].st) update(d[i]);
  }
  if (n.st == 1) n.st = 0;
  if (n.st == 2) exec(n);
}

// A failing node goes to its {:#try} block, if it is in one; else it logs,
// once per error, and leaves the rest to run.
const failed = new WeakMap();
function exec(n) {
  n.st = 0;
  const s = n.stops;
  if (s) (n.stops = null), run(s);
  const prev = observer;
  observer = n;
  n.run = ++runs;
  n.di = 0;
  try {
    const r = n.f();
    if (n.memo) {
      if (same(r, n.x)) return;
      n.x = r;
      const s = n.subs;
      if (s) for (let i = 0; i < s.length; i += 2) if (s[i].st == 1) s[i].st = 2;
    } else if (n.u && typeof r == 'function') (n.stops ||= []).push(r);
  } catch (e) {
    fail(n, e);
  } finally {
    if (n.deps && n.di < n.deps.length) unlink(n, n.di);
    observer = prev;
  }
}

function fail(n, e) {
  const sc = n.sc;
  if (sc?.b || failed.get(n) !== String(e)) report(sc, e), failed.set(n, String(e));
}

// An error goes to the {:#try} block its scope is in, else to the console.
const report = (sc, e) => (sc?.b ? sc.b(e) : console.error(e));

function dispose(n) {
  n.dead = 1;
  all.delete(n);
  if (n.deps) unlink(n, 0);
  const s = n.stops;
  if (s) (n.stops = null), run(s);
}

// A node: f runs (as n.f()) in the next batch, and again whenever what it
// read changes. An effect (fx) runs once the DOM has settled; a user's (u)
// may return its cleanup. It ends with its owner, a scope or an effect.
// One shape for all, with watch()'s fields.
function node(owner, f, fx = 0, u = 0) {
  const sc = owner?.sc || owner;
  const n = { f, fx, u, sc, id: ++ids, st: 0, deps: null, di: 0, stops: null, run: 0, dead: 0, ws: null, ep: epoch };
  if (owner) (owner.stops ||= []).push(n);
  if (!u && !sc?.c) all.add(n);
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
// Date, an element, a class instance) is left as it is. One handler for
// all; each object's signals are made as something reads them.
const metas = new WeakMap(); // object -> { p: its proxy, s: key -> Sig, k: its keys' Sig, v: any change's Sig }
const sigOf = (m, k) => {
  const s = (m.s ||= new Map());
  let x = s.get(k);
  if (!x) s.set(k, (x = new Sig(0)));
  return x;
};
const keysOf = (m) => (m.k ||= new Sig(0));
const verOf = (m) => (m.v ||= new Sig(0));
const bump = (s) => s && (s.v = s.x + 1);
function changed(m, k, added) {
  bump(m.s?.get(k));
  if (added) bump(m.k);
  bump(m.v);
}
const OBJ = {
  get(t, k, r) {
    if (k === RAW) return track(verOf(metas.get(t))), t;
    // Not methods: `list.map` reads the length and items it needs.
    if (observer && (Object.hasOwn(t, k) || !(k in t))) track(sigOf(metas.get(t), k));
    return proxy(Reflect.get(t, k, r));
  },
  has(t, k) {
    if (observer) track(keysOf(metas.get(t)));
    return k in t;
  },
  ownKeys(t) {
    if (observer) track(keysOf(metas.get(t)));
    return Reflect.ownKeys(t);
  },
  set(t, k, v) {
    const m = metas.get(t);
    const had = Object.hasOwn(t, k);
    const old = t[k];
    const n = t.length;
    t[k] = v;
    if (!had || !same(old, v)) changed(m, k, !had);
    if (Array.isArray(t) && t.length !== n && m.s) {
      bump(m.s.get('length'));
      for (let i = t.length; i < n; i++) bump(m.s.get(String(i)));
    }
    return true;
  },
  deleteProperty(t, k) {
    if (Object.hasOwn(t, k)) delete t[k], changed(metas.get(t), k, 1);
    return true;
  },
};
function proxy(x) {
  if (!x || typeof x != 'object' || proxied.has(x) || Object.isFrozen(x)) return x;
  let m = metas.get(x);
  if (m) return m.p;
  const proto = Object.getPrototypeOf(x);
  // A Map or a Set is state once extra.js is in (a script that makes one
  // imports it).
  const coll = x instanceof Map || x instanceof Set;
  if (coll ? !X.coll : proto && proto != Object.prototype && proto != Array.prototype) return x;
  m = { p: new Proxy(x, coll ? X.coll : OBJ), s: null, k: null, v: null };
  proxied.add(m.p);
  metas.set(x, m);
  return m.p;
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

// A store kept in localStorage: extra.js has it, and a module or a lib
// file that uses it imports that.
export const persisted = (key, initial) => X.persisted(key, initial);

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

// A live search's test: `items.filter((i) => matches(i.name, q))`. Whether
// text has the query in it, whatever the case; an empty query matches all.
export const matches = (text, q) =>
  String(text ?? '').toLowerCase().includes(String(q ?? '').trim().toLowerCase());

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

// Calls f(value, first, el, a, scope) when get(L)'s value changed (as text,
// with flag 1), or when a start came between (a morph may have rewritten
// the DOM; not with flag 2). `first` is true then too. The element and name
// are passed, so the common f are shared, not made per binding. While a
// copy is bound (see copy()), its watchers are collected in `col` and run
// as one node: [get, L, f, el, a, flags, last value, ...].
let col = null;
function watch(sc, get, L, f, flags = 0, el = null, a = null) {
  if (col) return void col.push(get, L, f, el, a, flags, NONE);
  node(sc, WATCH).ws = [get, L, f, el, a, flags, NONE];
}

function WATCH() {
  const ws = this.ws;
  const again = this.ep !== epoch;
  this.ep = epoch;
  for (let i = 0; i < ws.length; i += 7) {
    observer = this;
    try {
      let v = ws[i](ws[i + 1]);
      const flags = ws[i + 5];
      if (flags & 1) v = str(v);
      const last = ws[i + 6];
      const first = last === NONE || (again && !(flags & 2));
      if (Object.is(v, last) && !first) continue;
      ws[i + 6] = v;
      observer = null; // untracked
      ws[i + 2](v, first, ws[i + 3], ws[i + 4], this.sc);
    } catch (e) {
      fail(this, e);
      if (this.dead) return;
    }
  }
}

// Binds a copy's elements with f, its watchers made one node.
function batch(sc, f) {
  const prev = col;
  const n = node(sc, WATCH);
  col = n.ws = [];
  try {
    f();
  } finally {
    col = prev;
  }
  if (!n.ws.length) n.dead = 1;
}

// Listens on `at` for the event types (names split by spaces) until sc ends.
function on(sc, at, types, f, o) {
  for (const t of types.split(' ')) {
    at.addEventListener(t, f, o);
    sc.stops.push(() => at.removeEventListener(t, f, o));
  }
}

// A scope owns cleanups (its nodes among them): one per instance, element
// and copy. `b` is the {:#try} block it is in; `c` says it is in a copy,
// whose nodes a morph does not touch.
const scope = (up, c) => ({ stops: [], b: up?.b, dead: 0, c: c || up?.c || 0 });

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
  // Parsed by wisp.js already, if it is there; ours to change now.
  const { m = {}, i = [], r = null, p = {} } = json ? json.__j || JSON.parse(json.textContent) : {};
  if (json) json.__j = null;
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
  // The attributes are read here, once (`__d` and `__l` are data-w and data-wl).
  const els = {};
  for (const el of document.querySelectorAll('[data-w]')) {
    const w = (el.__d = el.getAttribute('data-w'));
    el.__l = el.getAttribute('data-wl');
    const dot = w.indexOf('.');
    if (dot > 0) (els[w.slice(0, dot)] ||= []).push(el);
  }
  // waiting: instance -> what starts once it has (its +page.js loaded, or
  // it hydrated), so that getContext finds what its script sets.
  const old = new Map(live.map((o) => [o.I, o])); // the last start's, by place
  Object.assign(cur, { els, my, byI: {}, waiting: {}, kept: new Set(), old, reset: document.querySelector('[data-wisp-reset]') });
  live = [];
  list.forEach(begin);
  for (const o of old.values()) if (!cur.kept.has(o)) destroy(o);
  loaded = true;
  for (const n of all) mark(n, 2);
}

function begin(rec) {
  const { I, id, P, blob, how } = rec;
  const { els, byI, waiting, kept, old, my, reset } = cur;
  const def = defs.get(id);
  const mine = els[I] || [];
  if (waiting[P]) return waiting[P].push(rec);
  // Kept: the instance whose element this is, or failing that (its
  // elements were all replaced) the one at the same place in the list.
  let inst = null;
  if (def && !(reset && mine.some((el) => el.closest('[data-wisp-reset]')))) {
    const ok = (o) => o && o.def === def && !kept.has(o) && old.get(o.I) === o && !o.sc.dead;
    inst = mine.map((el) => recs.get(el)?.inst).find(ok) || (ok(old.get(I)) ? old.get(I) : null);
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
    const { __d: w, __l: wl } = el;
    let r = recs.get(el);
    if (!(r && r.inst === inst && r.w === w && r.wl === wl && inst.recs.has(r))) {
      if (r) stopRec(r);
      r = { inst, w, wl, sc: scope(), el };
      recs.set(el, r);
      inst.recs.add(r);
      const quiet = !loaded || seen.has(el);
      const go = () => setup(r.sc, inst, el, +w.slice(w.indexOf('.') + 1), wl ? JSON.parse(wl) : {}, quiet);
      const wt = waits(el);
      wt ? wt.then(() => r.sc.dead || go()) : go();
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
  __wisp_eq: eq,
  untrack,
  tick,
  derived,
  store,
  persisted,
  goto,
  invalidate,
  matches,
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
    addEventListener: (type, f, options) => on(sc, window, type, wrap(f), options),
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
      report(sc, e);
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
// per template: [path, group, loop values, the block's own content]. The
// content is made lighter first: a {:hole}'s anchor and end become one
// text node, which the hole writes; whitespace between table rows and
// cells goes; and a block inside moves its content to a template of its
// own (its copies' clones point there), so it is not cloned with each copy.
const TABLE = /^(TABLE|THEAD|TBODY|TFOOT|TR|COLGROUP)$/;
const ROW = /^(TR|TD|TH|THEAD|TBODY|TFOOT|CAPTION|COLGROUP|COL)$/;
function paths(tpl) {
  if (tpl.__p) return tpl.__p;
  const top = tpl.content;
  const rows = [...top.children].some((c) => ROW.test(c.nodeName));
  const trim = (n) => {
    for (let c = n.firstChild, next; c; c = next) {
      next = c.nextSibling;
      if (c.nodeType == 3 && (n == top ? rows : TABLE.test(n.nodeName)) && !c.data.trim()) c.remove();
      else if (c.nodeType == 1) trim(c);
    }
  };
  trim(top);
  const out = [];
  const walk = (n, path) => {
    let i = 0;
    for (let c = n.firstChild; c; c = c.nextSibling, i++) {
      if (c.nodeType != 1) continue;
      const p = [...path, i];
      const w = c.dataset.w;
      const wl = c.dataset.wl && JSON.parse(c.dataset.wl);
      const end = c.nextSibling;
      if (c.localName == 'template' && w && !c.content.firstChild && end?.nodeType == 8 && !end.data) {
        const t = document.createTextNode('');
        end.remove();
        c.replaceWith(t);
        out.push([p, w, wl, null]);
        c = t;
      } else {
        let own = null;
        if (c.localName == 'template' && w) (own = document.createElement('template')).content.append(c.content);
        if (w || c.hasAttribute('data-wslot')) out.push([p, w, wl, own]);
        walk(c, p);
      }
    }
  };
  walk(top, []);
  // An end marker only where what comes last could grow after it.
  const last = top.lastChild;
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
  batch(sc, () => {
    for (const el of els) {
      const wl = el.getAttribute('data-wl');
      bindEl(el, sc, inst, L, true, el.getAttribute('data-w'), wl && JSON.parse(wl));
    }
  });
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

// A copy of tpl's content, bound to inst, put in c: a fragment, its first
// node and its last (an end marker, so what is later put inside the range
// moves and goes with it), and whether it has elements that play out (t)
// or slide (f).
function copy(tpl, sc, inst, L, quiet, c = {}) {
  tpl = tpl.__src || tpl;
  const ps = paths(tpl);
  const frag = tpl.content.cloneNode(true);
  const found = [];
  for (const [p, , , own] of ps) {
    let n = frag;
    for (const i of p) {
      n = n.firstChild;
      for (let k = 0; k < i; k++) n = n.nextSibling;
    }
    if (own) n.__src = own;
    found.push(n);
  }
  if (tpl.__end) frag.append(document.createComment(''));
  for (let n = frag.firstChild; n; n = n.nextSibling) n.__w = 1;
  const o = X.outs;
  const f = X.flips;
  batch(sc, () => {
    for (let k = 0; k < ps.length; k++) bindEl(found[k], sc, inst, L, quiet, ps[k][1], ps[k][2]);
  });
  c.first = frag.firstChild;
  c.last = frag.lastChild;
  c.frag = frag;
  c.t = X.outs != o;
  c.f = X.flips != f;
  return c;
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


// The root's listener for delegated events: the handlers of each element
// from the target up, as if on each, until one stops the event.
const rooted = new Set();
function delegate(e) {
  for (let n = e.target; n; n = n.parentNode) {
    const hs = n.__on;
    if (!hs) continue;
    for (const [type, f, sc, L] of hs) {
      if (type != e.type || sc.dead) continue;
      Object.defineProperty(e, 'currentTarget', { configurable: true, value: n });
      L ? untrack(() => f(L, e)) : f(e);
    }
    if (e.cancelBubble) break;
  }
}

// What watchers write.
const text = (v, first, el) => (el.textContent = v);
const data = (v, first, el) => (el.data = v);
const style = (v, first, el, a) => (v == null || v === false ? el.style.removeProperty(a) : el.style.setProperty(a, v));
function toggle(v, first, el, a) {
  el.__cls?.set(a, !!v);
  el.classList.toggle(a, !!v);
}

// :attr and attr={:…}: false, null and undefined remove it; aria-* states
// are "true" or "false". A transition plays as `hidden` turns off, and
// before it turns on.
function attr(v, first, el, a, sc) {
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
  if (a != 'hidden' || first || !X.hide) return put();
  X.hide(el, s != null, put, sc);
}

function binding(sc, inst, el, L, quiet, bnd) {
  const [kind, a, b, c] = bnd;
  switch (kind) {
    case 'on': {
      // b: the modifiers, as the compiler worked them out: bits (prevent 1,
      // stop 2, once 4, self 8, capture 16, passive 32, window 64, document
      // 128, outside 256, ctrl 512, shift 1024, alt 2048, meta 4096, and
      // 8192 for an event the root handles), then the keys (e.key, lower
      // case) and the debounce time.
      const [, , , , keys, ms] = bnd;
      const root = b & 8192 && el.nodeType == 1;
      if (root) {
        if (!rooted.has(a)) rooted.add(a), document.documentElement.addEventListener(a, delegate);
        // A kept element bound again drops its old handlers.
        el.__on = el.__on ? el.__on.filter((x) => !x[2].dead) : [];
        // Without modifiers, no function of its own: the root calls c(L, e).
        if (b == 8192) return void el.__on.push([a, c, sc, L]);
      }
      let timer, spent;
      const h = (e) => {
        if (b & 8 && e.target !== el) return;
        if (b & 256 && e.composedPath().includes(el)) return; // the path survives the target's removal
        if (keys && !keys.includes(e.key?.toLowerCase())) return;
        if ((b & 512 && !e.ctrlKey) || (b & 1024 && !e.shiftKey) || (b & 2048 && !e.altKey) || (b & 4096 && !e.metaKey)) return;
        // .once by hand, so an event the filters above turn away does not use it up.
        if (spent) return;
        spent = b & 4;
        if (b & 1) e.preventDefault();
        if (b & 2) e.stopPropagation();
        if (!ms) return untrack(() => c(L, e));
        clearTimeout(timer);
        timer = setTimeout(() => sc.dead || c(L, e), ms);
      };
      if (root) return void el.__on.push([a, h, sc]);
      return on(sc, b & 64 ? window : b & 384 ? document : el, a, h, { capture: !!(b & 16), passive: !!(b & 32) });
    }
    case 'bind':
      return a == 'this' ? c(L, el) : a == 'value' || a == 'checked' ? bind(sc, el, L, a, b, c) : X.bind(sc, el, L, a, b, c);
    case 'attr':
      return watch(sc, b, L, attr, 0, el, a);
    case 'text':
      return watch(sc, a, L, text, 1, el);
    case 'hole': {
      // The anchor's value goes in one text node between it and the end
      // comment after it; a morph may have added or taken nodes there.
      // In a copy, the anchor is the text node itself (see paths()).
      if (el.nodeType == 3) return watch(sc, a, L, data, 1, el);
      let t = null;
      const put = (s) => {
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
      };
      return watch(sc, a, L, put, 1);
    }
    case 'class':
      // Kept where the module writes class attributes too (see attr()).
      bnd.k ??= inst.g.some((g) => g.some((x) => x[0] == 'attr' && x[1] == 'class'));
      if (bnd.k) el.__cls ||= new Map();
      return watch(sc, b, L, toggle, 0, el, a);
    case 'style':
      return watch(sc, b, L, style, 0, el, a);
    case 'use': {
      let r;
      sc.stops.push(() => (typeof r == 'function' ? r() : r?.destroy?.()));
      return watch(sc, b || (() => {}), L, (v, first) => (first ? (r = a(L)(el, v)) : r?.update?.(v)), 2);
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
  const multi = el.multiple && el.localName == 'select';
  const read = () =>
    a == 'checked' ? el.checked : multi ? [...el.selectedOptions].map((o) => o.value) : /^(number|range)$/.test(el.type) && el.value !== '' ? +el.value : el.value;
  const typed = a == 'checked' ? el.checked !== el.defaultChecked : 'defaultValue' in el && el.value !== el.defaultValue;
  if (typed || get(L) == null) set(L, read());
  on(sc, el, a == 'checked' || el.localName == 'select' ? 'change' : 'input', () => set(L, read()));
  // A form reset (wisp.js resets a form its post succeeded with) moves
  // the field to its new default; the variable follows it.
  if (el.form) on(sc, el.form, 'reset', () => queueMicrotask(() => set(L, read())));
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
  const fresh = (key, item, i) => {
    const cl = Object.create(P);
    return { key, L: cl, $: (cl[S] = [new Sig(item), names[1] && new Sig(i)]), sc: scope(sc, 1), i: -1, first: null, last: null, frag: null, t: false, f: false };
  };
  const draw = (q) => (c) => copy(tpl, c.sc, inst, c.L, q, c);
  node(sc, () => {
    const v = get(L);
    // A state array is read whole, as one signal, not item by item.
    const raw = kind == 'each' && v?.[RAW];
    const items = Array.isArray(raw) ? raw.map(proxy) : kind == 'if' ? (v ? [0] : []) : kind == 'key' ? [v] : typeof v == 'number' ? Array.from({ length: v }, (_, i) => i) : [...(v ?? [])];
    // Keys are tracked: an item whose key changes in place moves.
    const keys = kind == 'key' ? items : keyOf ? items.map((item, i) => (names[0] && (tmp[names[0]] = item), names[1] && (tmp[names[1]] = i), keyOf(tmp))) : null;
    untrack(() => {
      const m = list.length;
      // The server's copies: the first draw takes them over in order; after
      // a morph that kept this block, the new page's go.
      const pre = [];
      if (first) for (let c, at = tpl; (c = painted(at)); at = c.last) pre.push(c);
      else {
        drop(list.at(-1)?.last || tpl);
        // The same keys in the same order, and perhaps more after them: none
        // moves, the values are set, and the new ones go in at the end.
        let k = 0;
        while (!X.flips && k < m && k < items.length && (keys ? keys[k] : k) === list[k].key) k++;
        if (k == m) {
          for (k = 0; k < m; k++) list[k].$[0].v = items[k];
          const add = items.slice(m).map((item, i) => fresh(keys ? keys[m + i] : m + i, item, m + i));
          if (add.length) order(tpl.parentNode, (list.at(-1)?.last || tpl).nextSibling, add, draw(false));
          list = list.concat(add);
          return;
        }
      }
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
        const n = fresh(key, item, i);
        // A painted copy is taken over where it is.
        if (pre[i]) Object.assign(n, adopt(pre[i], n.sc, inst, n.L), { i });
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
      order(tpl.parentNode, tail, next, draw(quiet && first));
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
  ...{ Sig, node, watch, scope, end, untrack, clones, binding, range, cls, css, track, proxy, same, proxied, shared, sub, on, report },
  ...{ defs, instance, script, adopt, painted, place, drop, RAW, metas, sigOf, keysOf, verOf, changed, bump },
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
