// extra.js: the less used half of Wisp's browser runtime, served as
// /_app/c/extra.js to the modules that use it (see `codegen`), so a page
// that does not pays nothing for it: transitions and animate:flip,
// {:#await} and {:#try}, <wisp:element>, {:...spread}, client:* on an
// element, bind: on anything but value and checked, components drawn in
// the browser, use:enhance, $state.snapshot, persisted stores, Maps and
// Sets as state, tweened, spring and crossfade, and pages' own snapshots.
// It adds its kinds of binding and helpers to live.js's `__wisp`, which
// calls them.
import { __wisp as X, page, store } from 'wisp';

const { Sig, node, watch, scope, end, untrack, clones, binding, range, attr, track, proxy, same, proxied, on, report } = X;
const { defs, instance, script, adopt, painted, place, drop, RAW, metas, sigOf, keysOf, verOf, changed, bump } = X;
const trans = new WeakMap(); // element -> { i, o }: its in and out transitions, [kind, options]
const anims = new WeakMap(); // element -> its running animation
const reduce = matchMedia('(prefers-reduced-motion: reduce)');

// ---- transitions ------------------------------------------------------------

// transition:x (both ways), in:x, out:x: [kind, options, 0 | 1 in | 2 out];
// a kind that is not a name is the script's function.
X.transition = (sc, inst, el, L, quiet, [, a, b, c]) => {
  const t = trans.get(el) || {};
  const x = [typeof a == 'function' ? a(L) : a, () => b?.(L)];
  if (c != 2) t.i = x;
  if (c != 1) (t.o = x), X.outs++;
  trans.set(el, t);
};

X.animate = (sc, inst, el) => {
  el.__flip = 1;
  X.flips++;
};

// An element drawn after the page loaded plays in, unless it is hidden.
X.enter = (sc, el) => {
  if (!trans.get(el)?.i) return;
  let first = true;
  node(sc, () => {
    if (first && !el.hidden) play(el);
    first = false;
  });
};

// `hidden` turning off plays in; turning on plays out, then hides.
X.hide = (el, on, put, sc) => {
  if (!trans.has(el)) return put();
  if (on) return play(el, true, () => sc.dead || put());
  put();
  play(el);
};

// A copy leaves: its transitions play out first.
X.leave = (sc, c) => {
  const nodes = range(c);
  const gone = () => sc.dead || nodes.forEach((n) => n.remove());
  const out = nodes.flatMap((n) => (n.nodeType == 1 ? [n, ...n.querySelectorAll('*')] : [])).filter((el) => trans.get(el)?.o);
  let left = out.length;
  if (!left) return gone();
  for (const el of out) play(el, true, () => --left || gone());
};

// animate:flip: where each element of the copies that stay was; the
// function returned slides them from there once they are in order.
X.flip = (list, gone) => {
  const rects = new Map();
  for (const c of list) if (c.f && !gone.has(c.key)) for (const n of range(c)) if (n.__flip) rects.set(n, n.getBoundingClientRect());
  return () => {
    for (const [el, r] of rects) {
      if (!el.isConnected || reduce.matches) continue;
      const now = el.getBoundingClientRect();
      const dx = r.left - now.left;
      const dy = r.top - now.top;
      if (dx || dy) el.animate([{ transform: `translate(${dx}px, ${dy}px)` }, { transform: 'none' }], { duration: 200, easing: 'ease-out' });
    }
  };
};

// The frames of the built-in transitions, from hidden to shown.
function builtin(name, el, o) {
  if (name == 'slide') return [{ opacity: 0, height: 0, overflow: 'hidden' }, { height: el.offsetHeight + 'px', overflow: 'hidden' }];
  if (name == 'blur') return [{ opacity: 0, filter: `blur(${o.amount ?? 5}px)` }, {}];
  return [{ fade: { opacity: 0 }, scale: { opacity: 0, transform: `scale(${o.start ?? 0.95})` }, fly: { opacity: 0, transform: `translate(${o.x ?? 0}px, ${o.y ?? 8}px)` } }[name] || {}, {}];
}

// A custom transition's css(t, u) as frames: sampled, the easing done here.
function sampled(r) {
  const n = Math.max(2, Math.ceil(r.duration / 16));
  const out = [];
  for (let i = 0; i <= n; i++) {
    const t = (r.easing || ((x) => x))(i / n);
    out.push(Object.fromEntries(r.css(t, 1 - t).split(';').filter((d) => d.includes(':')).map((d) => {
      const k = d.slice(0, d.indexOf(':')).trim();
      return [k.startsWith('--') ? k : k.replace(/-([a-z])/g, (_, c) => c.toUpperCase()), d.slice(d.indexOf(':') + 1).trim()];
    })));
  }
  return out;
}

// Plays el's transition in, or out (the same frames reversed); done runs
// when it finishes, not when a newer play on el cancels it. A custom one,
// fn(el, options, { direction }), returns { duration, delay, easing, css }
// or { tick(t, u) }.
function play(el, out, done) {
  anims.get(el)?.cancel();
  const t = trans.get(el)?.[out ? 'o' : 'i'];
  if (!t) return done?.();
  const o = untrack(t[1]) || {};
  let frames;
  let timing = { duration: o.duration ?? 150, delay: o.delay || 0, easing: o.easing || 'ease-out' };
  if (typeof t[0] == 'function') {
    const r = t[0](el, o, { direction: out ? 'out' : 'in' }) || {};
    timing = { duration: r.duration ?? 300, delay: r.delay || 0, easing: 'linear' };
    if (r.tick) {
      const start = performance.now() + timing.delay;
      let id;
      const step = (now) => {
        const p = Math.min(1, Math.max(0, (now - start) / timing.duration));
        const e = (r.easing || ((x) => x))(out ? 1 - p : p);
        r.tick(e, 1 - e);
        if (p < 1) id = requestAnimationFrame(step);
        else done?.();
      };
      id = requestAnimationFrame(step);
      return anims.set(el, { cancel: () => cancelAnimationFrame(id) });
    }
    frames = r.css ? sampled({ ...r, duration: timing.duration }) : [{}, {}];
  } else frames = builtin(t[0], el, o);
  const anim = el.animate(frames, { ...timing, duration: reduce.matches ? 0 : timing.duration, direction: out ? 'reverse' : 'normal' });
  anims.set(el, anim);
  if (done) anim.onfinish = done;
}

// ---- blocks -----------------------------------------------------------------

// {:#await p}: one copy, whose `__aw` is { k: 0 pending | 1 then | 2 catch, x }.
X.await = (sc, inst, el, L, quiet, [, a]) => {
  const st = new Sig({ k: 0 });
  let token = 0;
  node(sc, () => {
    const p = a(L);
    const my = ++token;
    if (!p || typeof p.then != 'function') return void (st.v = { k: 1, x: p });
    if (st.x.k) st.v = { k: 0 };
    p.then((x) => my == token && (st.v = { k: 1, x }), (x) => my == token && (st.v = { k: 2, x }));
  });
  clones(sc, inst, el, L, quiet, 'each', () => [st.v], ['__aw']);
};

// {:#try}: one copy, whose `__tr` is { f: 1, e } once what it draws failed.
// An error while the failure shows goes to the block around; `reset()`,
// in `{:catch}`, draws the body again.
X.try = (sc, inst, el, L, quiet) => {
  const st = new Sig({});
  const inner = scope(sc);
  inner.b = (e) => (!st.x.f ? (st.v = { f: 1, e }) : report(sc, e));
  const R = Object.create(L);
  R.reset = () => (st.v = {});
  clones(inner, inst, el, R, quiet, 'each', () => [st.v], ['__tr']);
};

// <wisp:element this={:tag}>: when the tag changes, a new element takes the
// old one's place, attributes and children, and the other directives.
X.tag = (sc, inst, el, bs, L, quiet) => {
  let inner = null;
  watch(sc, bs[0][1], L, (tag) => {
    tag = String(tag || 'div').toLowerCase();
    if (inner) end(inner);
    if (tag != el.localName) {
      const next = document.createElementNS(el.namespaceURI, tag);
      for (const { name, value } of el.attributes) next.setAttribute(name, value);
      next.append(...el.childNodes);
      next.__w = el.__w;
      el.replaceWith(next);
      el = next;
    }
    inner = scope(sc);
    for (const b of bs.slice(1)) binding(inner, inst, el, L, quiet, b);
  });
};

// What an attribute holds that escaping does not make safe, in any case:
// on*, srcdoc, an <animate>/<set>'s to from values by, a <meta>'s
// http-equiv and content; a tag of '' is any (`contexts::holds_script`).
const held = (tag, k) =>
  /^(on|srcdoc$)/i.test(k) || (/^(animate|set|)$/i.test(tag) && /^(to|from|values|by)$/i.test(k)) || (/^(meta|)$/i.test(tag) && /^(http-equiv|content)$/i.test(k));

// {:...attrs}: each key an attribute, set as `attr={:…}` sets it (a URL
// that would run script is blocked), a function under on* a listener. Any
// other held key (see above) is left out, as the server's first paint
// leaves it.
X.spread = (sc, inst, el, L, quiet, [, a]) => {
  let had = {};
  watch(sc, a, L, (v, first) => {
    v = v || {};
    for (const k in had) if (!(k in v)) k.startsWith('on') ? typeof had[k] == 'function' && el.removeEventListener(k.slice(2), had[k]) : held(el.localName, k) || el.removeAttribute(k);
    for (const k in v) {
      const x = v[k];
      if (!k.startsWith('on')) held(el.localName, k) || attr(x, first, el, k, sc);
      else if (had[k] !== x) {
        if (typeof had[k] == 'function') el.removeEventListener(k.slice(2), had[k]);
        if (typeof x == 'function') el.addEventListener(k.slice(2), x);
      }
    }
    had = { ...v };
  });
};

// client:<how> on an element: it, and what is inside it (see live.js's
// waits()), start when `v` it is near the viewport, `i` the browser is
// idle, `x` its first pointer, focus or key comes (a click is replayed
// once it is bound), `m(query)` the query matches; `n` never. Then `go`.
X.wait = (el, how, go) => {
  if (!el.__wait) {
    X.waiting++;
    el.__wait = new Promise((ok) => {
      if (how == 'v') {
        const io = new IntersectionObserver((es) => es.some((e) => e.isIntersecting) && (io.disconnect(), ok()), { rootMargin: '200px' });
        io.observe(el);
      } else if (how == 'i') (window.requestIdleCallback || setTimeout)(ok, { timeout: 2000 });
      else if (how[0] == 'm') {
        const mq = matchMedia(how.slice(1));
        if (mq.matches) ok();
        else mq.addEventListener('change', () => mq.matches && ok());
      } else if (how == 'x') {
        const ac = new AbortController();
        const f = (e) => {
          ac.abort();
          ok();
          if (e.type != 'click') return;
          e.preventDefault();
          e.stopImmediatePropagation();
          setTimeout(() => e.target.dispatchEvent(new MouseEvent('click', e))); // once bound
        };
        for (const t of ['pointerdown', 'focusin', 'keydown', 'click']) el.addEventListener(t, f, { capture: true, signal: ac.signal });
      }
    }).then(() => ((el.__wait = null), (el.__woke = 1), X.waiting--));
  }
  el.__wait.then(go);
};

// ---- bind: ------------------------------------------------------------------

// The event that tells of a change, where it is not `input`; sizes, which a
// ResizeObserver reports; and what only the element sets.
const EVENTS = { files: 'change', group: 'change', open: 'toggle', currentTime: 'timeupdate', paused: 'play pause', volume: 'volumechange', muted: 'volumechange', playbackRate: 'ratechange', duration: 'durationchange', innerWidth: 'resize', innerHeight: 'resize', outerWidth: 'resize', outerHeight: 'resize', scrollX: 'scroll', scrollY: 'scroll', online: 'online offline', fullscreenElement: 'fullscreenchange', visibilityState: 'visibilitychange', activeElement: 'focusin focusout' };
const SIZES = /^(client|offset)(Width|Height)$|^(contentRect|contentBoxSize|borderBoxSize|devicePixelContentBoxSize)$/;
const READONLY = /^(files|duration|buffered|seekable|played|ended|readyState|videoWidth|videoHeight|naturalWidth|naturalHeight|innerWidth|innerHeight|outerWidth|outerHeight|online|devicePixelRatio|fullscreenElement|visibilityState|activeElement)$/;

// bind:group, bind:files, bind:clientWidth, bind:open, bind:innerHTML, a
// media element's, the window's and the document's: the property and the
// variable, both ways where the element lets the variable set it.
X.bind = (sc, el, L, a, get, set) => {
  if (SIZES.test(a)) {
    const size = /Size$|Rect$/.test(a);
    const ro = new ResizeObserver(([e]) => set(L, size ? e[a] : el[a]));
    ro.observe(el);
    if (!size) set(L, el[a]);
    return void sc.stops.push(() => ro.disconnect());
  }
  const read =
    a == 'group'
      ? () => (el.type == 'checkbox' ? [...document.getElementsByName(el.name)].filter((i) => i.checked).map((i) => i.value) : el.checked ? el.value : get(L))
      : a == 'online'
        ? () => navigator.onLine
        : () => el[a];
  const mine = READONLY.test(a) || el === window || el === document;
  if (mine || get(L) == null) set(L, read());
  on(sc, el, EVENTS[a] || 'input', () => set(L, read()));
  if (mine && a != 'scrollX' && a != 'scrollY') return;
  node(sc, () => {
    const v = get(L);
    untrack(() => {
      if (a == 'group') el.checked = el.type == 'checkbox' ? !!v?.includes?.(el.value) : v === el.value;
      else if (a == 'scrollX' || a == 'scrollY') v != null && v !== read() && scrollTo(a == 'scrollX' ? v : scrollX, a == 'scrollY' ? v : scrollY);
      else if (a == 'paused') v !== el.paused && (v ? el.pause() : el.play());
      else if (read() !== v) el[a] = a.startsWith('inner') || a == 'textContent' ? (v ?? '') : v;
    });
  });
};

// {:@html expr}: the markup after the anchor, up to its <!--h--> end comment,
// made again when the value changes. Not escaped, as on the server.
X.html = (sc, inst, el, L, quiet, [, a]) =>
  watch(
    sc,
    a,
    L,
    (v) => {
      for (let n; (n = el.nextSibling) && !(n.nodeType == 8 && n.data == 'h'); ) n.remove();
      el.after(Object.assign(document.createElement('template'), { innerHTML: v }).content);
    },
    1,
  );

// ---- components, forms, collections -------------------------------------------

// A component the browser renders, after its anchor: a new instance of its
// module with the props from the page's code. Its script runs once; props
// that change go in as they do, and a `bind:` prop it changes comes back out.
X.comp = (sc, parent, anchor, L, quiet, [, id, props, binds, events]) => {
  const def = defs.get(id);
  // A component rendering itself (a tree) ends with its data; this stops
  // one that would not.
  const depth = (parent.depth || 0) + 1;
  if (depth > 64) return void console.error(`component ${id} nests more than 64 deep`);
  const child = instance(def, id, anchor, parent, depth);
  child.sc.b = sc.b;
  child.sc.c = 1; // all it draws is a copy
  child.slot = { tpl: anchor, inst: parent, L };
  for (const [name, f] of events) child.events[name] = (v) => untrack(() => f(L, v));
  const where = untrack(() => {
    try {
      script(child, { ...props(L) });
    } catch (e) {
      report(sc, e);
    }
    def.tpl ||= Object.assign(document.createElement('template'), { innerHTML: def.html || '' });
    return adopt(painted(anchor), child.sc, child, {}) || place(anchor, def.tpl, child.sc, child, {}, quiet);
  });
  sc.stops.push(() => (end(child.sc), range(where).forEach((n) => n.remove())));
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
};

// persisted(key, initial): a store kept in localStorage under `key`, and in
// step across tabs. It is saved whenever it changes, in place too. One per
// key: a script that asks again (each time it starts) gets the same.
const saved = new Map();
X.persisted = (key, initial) => {
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
  X.sub(() => JSON.stringify(s.value), (j) => {
    if (j === json) return;
    json = j;
    try {
      localStorage.setItem(key, j);
    } catch {}
  });
  addEventListener('storage', (e) => e.key === key && (s.value = load((json = e.newValue))));
  saved.set(key, s);
  return s;
};

// $state.snapshot: a plain copy, for structuredClone, a library or a log.
function snap(x) {
  if (!proxied.has(x)) return x;
  if (x instanceof Map) return new Map([...x].map(([k, v]) => [k, snap(v)]));
  if (x instanceof Set) return new Set([...x].map(snap));
  return Array.isArray(x) ? x.map(snap) : Object.fromEntries(Object.entries(x).map(([k, v]) => [k, snap(v)]));
}
X.shared.__wisp_snap = snap;

// use:enhance="fn" on a form: fn({ form, formData, submitter, action, cancel })
// runs as it is sent (for pending state and optimistic updates) and may
// return fn(result), run with { ok, status, data, location } after the
// page has been updated. `data` is the action's JSON answer, if any.
X.shared.enhance = (form, f) => {
  let after;
  const ac = new AbortController();
  const listen = (type, g) => form.addEventListener(type, g, { signal: ac.signal });
  form.__wispEnhance = true;
  listen('wisp:submit', (e) => {
    const { data: formData, submitter, action } = e.detail;
    after = typeof f == 'function' && f({ form, formData, submitter, action, cancel: () => e.preventDefault() });
  });
  listen('wisp:result', (e) => {
    page.value = { ...page.value, form: e.detail.data };
    Promise.resolve(after).then((g) => typeof g == 'function' && g(e.detail));
  });
  return () => ac.abort();
};

// A Map or a Set in state: get and has track their key, size and
// iteration the keys, anything else any change; set, add, delete and
// clear tell what read them.
X.coll = {
  get(t, k) {
    const m = metas.get(t);
    if (k === RAW) return track(verOf(m)), t;
    if (k == 'size') return track(keysOf(m)), t.size;
    const f = t[k];
    if (typeof f != 'function') return f;
    if (k == 'get' || k == 'has') return (key) => (track(sigOf(m, key)), k == 'has' ? t.has(key) : proxy(t.get(key)));
    if (k == 'set' || k == 'add')
      return (key, v) => {
        const had = t.has(key);
        const old = t.get?.(key);
        f.call(t, key, v);
        if (!had || !same(old, v)) changed(m, key, !had);
        return m.p;
      };
    if (k == 'delete') return (key) => t.delete(key) && (changed(m, key, 1), true);
    if (k == 'clear') return () => t.size && (t.clear(), m.s?.forEach(bump), changed(m, null, 1));
    return (...a) => (track(verOf(m)), f.apply(t, a));
  },
};

// ---- motion -------------------------------------------------------------------

// A number, an array or an object of numbers, as a list of them and back.
const nums = (v) => (typeof v == 'number' ? [v] : Object.values(v));
const shape = (v, a) => (typeof v == 'number' ? a[0] : Array.isArray(v) ? a : Object.fromEntries(Object.keys(v).map((k, i) => [k, a[i]])));
const cubic = (t) => 1 - (1 - t) ** 3;

// tweened(value, { duration = 400, delay, easing }): a store whose value
// runs to what it is set to (`t.value = 5`, `t.set(5, { duration: 0 })`),
// as numbers, arrays or objects of them. set() resolves once it is there
// (or something newer replaced it). Reduced motion jumps.
X.shared.tweened = (v, o = {}) => {
  const s = store(v);
  let id = 0;
  let stop;
  let to = v;
  const set = (x, p = {}) => {
    cancelAnimationFrame(id);
    stop?.();
    const a = nums(s.value);
    const b = nums((to = x));
    const d = reduce.matches ? 0 : (p.duration ?? o.duration ?? 400);
    const e = p.easing || o.easing || cubic;
    const t0 = performance.now() + (p.delay ?? o.delay ?? 0);
    return new Promise((done) => {
      stop = done;
      const step = (now) => {
        const k = d > 0 ? Math.min(1, Math.max(0, (now - t0) / d)) : 1;
        s.value = k < 1 ? shape(x, a.map((n, i) => n + (b[i] - n) * e(k))) : x;
        if (k < 1) id = requestAnimationFrame(step);
        else done();
      };
      d > 0 ? (id = requestAnimationFrame(step)) : step(t0);
    });
  };
  return {
    get value() {
      return s.value;
    },
    set value(x) {
      set(x);
    },
    set,
    update: (f, p) => set(f(to), p),
    subscribe: s.subscribe,
  };
};

// spring(value, { stiffness = 0.15, damping = 0.8, precision = 0.01 }): a
// store whose value is pulled to its target, with momentum, as numbers,
// arrays or objects of them. set(x, { hard: true }) jumps; set() resolves
// at rest.
X.shared.spring = (v, o = {}) => {
  const s = store(v);
  let id = 0;
  let to = v;
  let vel = nums(v).map(() => 0);
  let last = 0;
  let wake;
  const step = (now) => {
    const f = Math.min((now - last) / 16.7, 4);
    last = now;
    const b = nums(to);
    let rest = 1;
    const a = nums(s.value).map((n, i) => {
      const d = b[i] - n;
      vel[i] += (d * (o.stiffness ?? 0.15) - vel[i] * (o.damping ?? 0.8)) * f;
      if (Math.abs(vel[i]) < (o.precision ?? 0.01) && Math.abs(d) < (o.precision ?? 0.01)) return b[i];
      rest = 0;
      return n + vel[i] * f;
    });
    s.value = rest ? to : shape(to, a);
    if (rest) (vel = vel.map(() => 0)), (id = 0), wake?.();
    else id = requestAnimationFrame(step);
  };
  const set = (x, p = {}) => {
    wake?.();
    to = x;
    if (p.hard || reduce.matches) {
      cancelAnimationFrame(id);
      id = 0;
      vel = nums(x).map(() => 0);
      s.value = x;
      return Promise.resolve();
    }
    return new Promise((done) => {
      wake = done;
      if (!id) (last = performance.now()), (id = requestAnimationFrame(step));
    });
  };
  return {
    get value() {
      return s.value;
    },
    set value(x) {
      set(x);
    },
    set,
    update: (f, p) => set(f(to), p),
    subscribe: s.subscribe,
  };
};

// const [send, receive] = crossfade({ duration, easing }): transitions for
// an element that leaves one place and one that comes in another with the
// same key (`out:send={{ key: id }}`, `in:receive={{ key: id }}`): the
// newcomer starts where the leaver was. One with no partner fades.
X.shared.crossfade = (o = {}) => {
  const seen = [new Map(), new Map()];
  const one = (me) => (el, p) => {
    const r = el.getBoundingClientRect();
    const at = performance.now();
    const op = +getComputedStyle(el).opacity;
    seen[me].set(p.key, { r, at });
    return {
      duration: p.duration ?? o.duration ?? 400,
      easing: p.easing || o.easing || cubic,
      tick(t) {
        const w = seen[1 - me].get(p.key);
        const u = 1 - t;
        const st = el.style;
        if (t >= 1) return void (st.opacity = st.transform = st.transformOrigin = '');
        st.opacity = w && at - w.at < 100 ? op : t * op;
        if (!w || at - w.at >= 100) return;
        const a = w.r;
        st.transformOrigin = 'top left';
        st.transform = `translate(${u * (a.left - r.left)}px, ${u * (a.top - r.top)}px) scale(${t + (u * a.width) / (r.width || 1)}, ${t + (u * a.height) / (r.height || 1)})`;
      },
    };
  };
  return [one(0), one(1)];
};

// ---- snapshots ----------------------------------------------------------------

// `export const snapshot = { capture, restore }` in a script: what capture()
// gives is kept with the history entry (wisp.js keeps it, by module and
// order), and handed to restore() when the entry comes back by back,
// forward or a reload.
const snaps = new Set();
const tell = (type, d) => (document.dispatchEvent(new CustomEvent(type, { detail: d })), d);
X.snap = (inst) => {
  snaps.add(inst);
  const v = tell('wisp:restore', {}).s?.[inst.id]?.shift();
  try {
    if (v != null) inst.snap.restore?.(v);
  } catch (e) {
    console.error(e);
  }
};
document.addEventListener('wisp:capture', ({ detail: d }) => {
  for (const i of snaps) {
    if (i.sc.dead) snaps.delete(i);
    else
      try {
        (d.s[i.id] ||= []).push(i.snap.capture?.() ?? null);
      } catch (e) {
        console.error(e);
      }
  }
});
document.addEventListener('wisp:pop', () => snaps.forEach(X.snap));
