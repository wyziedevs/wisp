// Runs wisp.js against a small stand-in for the browser: the offline form
// queue, focus after a client navigation, and the navigation hooks and link
// attributes. Run by client.rs (`node client.js`); what is not here (real
// layout, scrolling, view transitions, morphing) needs a browser.
const assert = require('node:assert');
const fs = require('node:fs');
const path = require('node:path');
const src = fs.readFileSync(path.join(__dirname, '../src/client/wisp.js'), 'utf8');

const tick = () => new Promise((r) => setTimeout(r, 10));
const res = (body, type = 'text/html', status = 200, disposition = null) => ({
  ok: status < 400,
  status,
  redirected: false,
  headers: { get: (k) => (k == 'content-type' ? type : k == 'content-disposition' ? disposition : null) },
  text: async () => body,
  clone() { return this; },
});

// A fresh page with wisp.js running in it. `fetches` records each call.
function page({ online = true, scripts = [], vitals = null, views = null, main = null, cuts = null } = {}) {
  const win = new EventTarget();
  const doc = new EventTarget();
  const fetches = [];
  const calls = { scroll: [], say: [], head: [], beacon: [] };
  const h1 = Object.assign(new EventTarget(), {
    attrs: {},
    style: {},
    focused: 0,
    setAttribute(k, v) { this.attrs[k] = v; },
    removeAttribute(k) { delete this.attrs[k]; },
    focus(o) { this.focused++; this.opts = o; },
  });
  const body = { nodeType: 3, nodeValue: '', dataset: {}, children: [], append(e) { calls.say.push(e); } };
  Object.assign(doc, {
    scripts: [],
    head: { children: [], append: (e) => calls.head.push(e) },
    body,
    currentScript: null,
    title: '',
    activeElement: { blur() {} },
    getElementById: (id) => (id == 'wisp-loading' && views ? { text: JSON.stringify(views) } : null),
    querySelectorAll: (s) => (s.includes('wisp/') ? scripts : s.includes('data-wisp-cut') ? cuts || [] : []),
    querySelector: (s) => (s == 'h1' ? h1 : s == 'main' ? main : s.includes('data-wisp-cut') ? cuts?.[0] ?? null : s.includes('wisp-vitals') ? vitals : null),
    visibilityState: 'visible',
    createElement: () => Object.assign(new EventTarget(), { style: {}, attrs: {}, setAttribute(k, v) { this.attrs[k] = v; }, isConnected: true }),
    hidden: false,
  });
  const store = {};
  const nav = { onLine: online, sendBeacon: (u, b) => calls.beacon.push([u, b]) };
  const idle = [];
  const loc = { href: 'http://x.test/', origin: 'http://x.test', pathname: '/', assign: (u) => calls.assign = String(u) };
  class Anchor {}
  class Form {}
  Form.prototype.reset = () => {};
  Form.prototype.requestSubmit = () => {};
  const g = {
    document: doc,
    location: loc,
    navigator: nav,
    history: { state: null, replaceState(s, _, u) { this.state = s; if (u) loc.href = String(u); }, pushState(s, _, u) { this.state = s; loc.href = String(u); } },
    sessionStorage: Object.assign(store, { getItem: (k) => store[k] ?? null, setItem: (k, v) => { store[k] = v; }, removeItem: (k) => { delete store[k]; } }),
    addEventListener: (t, f) => win.addEventListener(t, f),
    requestIdleCallback: (f) => idle.push(f),
    scrollX: 0,
    scrollY: 0,
    // Navigations scroll instantly: `scrollTo({ left, top, behavior: 'instant' })`.
    scrollTo: (o) => (assert.equal(o.behavior, 'instant'), calls.scroll.push([o.left, o.top])),
    HTMLAnchorElement: Anchor,
    HTMLFormElement: Form,
    FormData: class { constructor(f) { this.d = f.data; } [Symbol.iterator]() { return this.d[Symbol.iterator](); } },
    DOMParser: class {
      parseFromString(html) {
        const t = /<title>(.*?)<\/title>/.exec(html);
        return { title: t ? t[1] : '', head: { children: [] }, body: { nodeType: 3, nodeValue: '', innerHTML: html }, querySelectorAll: () => [], querySelector: () => null, getElementById: () => null };
      }
    },
    fetch: async (u, o = {}) => {
      fetches.push({ url: String(u), method: o.method || 'GET', body: o.body && String(o.body), headers: o.headers });
      return g.reply(String(u), o);
    },
    reply: () => res('<title>Next</title><h1>Next</h1>'),
  };
  for (const k of Object.keys(g)) Object.defineProperty(globalThis, k, { value: g[k], configurable: true, writable: true });
  Object.defineProperty(globalThis, 'navigator', { value: nav, configurable: true, writable: true });
  new Function(src)();
  const event = (type, props, at = doc) => {
    const e = new Event(type, { cancelable: true, bubbles: true });
    for (const k in props) Object.defineProperty(e, k, { value: props[k] });
    at.dispatchEvent(e);
    return e;
  };
  const form = (attrs, data) => Object.assign(new EventTarget(), {
    data,
    getAttribute: (k) => (k == 'method' ? 'post' : k == 'action' ? '/save' : attrs[k] ?? null),
    hasAttribute: (k) => k in attrs,
    setAttribute() {},
    removeAttribute() {},
  });
  const link = (attrs = {}, href = '/next') => Object.assign(new Anchor(), {
    href: 'http://x.test' + href,
    rel: '',
    getAttribute: (k) => attrs[k] ?? null,
    hasAttribute: (k) => k in attrs,
    closest: (s) => (s == 'a[href]' ? link_ : s.startsWith('[data-wisp-') && s.slice(1, -1) in attrs ? link_ : null),
  });
  let link_;
  const click = (attrs, href) => event('click', { target: (link_ = link(attrs, href)), button: 0 });
  return { idle, win, doc, g, h1, fetches, calls, store, nav, event, form, click, loc, submit: (f) => event('submit', { target: f }) };
}

const tests = {
  async 'offline: a data-wisp-queue form waits, and is sent when online'() {
    const p = page({ online: false });
    const e = p.submit(p.form({ 'data-wisp-queue': '' }, [['a', '1'], ['b', 'x y']]));
    assert.ok(e.defaultPrevented);
    assert.equal(p.fetches.length, 0);
    assert.deepEqual(JSON.parse(p.store['wisp:q']), [['http://x.test/save', 'a=1&b=x+y']]);
    let sent = 0;
    p.doc.addEventListener('wisp:sent', () => sent++);
    p.nav.onLine = true;
    p.win.dispatchEvent(new Event('online'));
    await tick();
    const post = p.fetches[0];
    assert.equal(post.method, 'POST');
    assert.equal(post.url, 'http://x.test/save');
    assert.equal(post.body, 'a=1&b=x+y');
    assert.equal(post.headers['content-type'], 'application/x-www-form-urlencoded');
    assert.deepEqual(JSON.parse(p.store['wisp:q']), []);
    assert.equal(sent, 1);
    assert.equal(p.fetches[1].method, 'GET'); // the page again
  },
  async 'offline: another form is not queued'() {
    const p = page({ online: false });
    p.submit(p.form({}, [['a', '1']]));
    assert.equal(p.store['wisp:q'], undefined);
    assert.equal(p.fetches.length, 0);
  },
  async 'queue: a 500 keeps the posts, a 422 drops one, and the order holds'() {
    const p = page({ online: false });
    p.submit(p.form({ 'data-wisp-queue': '' }, [['n', '1']]));
    p.submit(p.form({ 'data-wisp-queue': '' }, [['n', '2']]));
    let status = 500;
    p.g.reply = () => res('', 'text/html', status);
    p.nav.onLine = true;
    p.win.dispatchEvent(new Event('online'));
    await tick();
    assert.equal(p.fetches.length, 1); // stopped at the first
    assert.equal(JSON.parse(p.store['wisp:q']).length, 2);
    status = 422;
    p.win.dispatchEvent(new Event('online'));
    await tick();
    assert.deepEqual(p.fetches.slice(1, 3).map((f) => f.body), ['n=1', 'n=2']);
    assert.deepEqual(JSON.parse(p.store['wisp:q']), []);
  },
  async 'a post that was answered is never posted again by the browser'() {
    const p = page();
    let again = 0;
    p.g.HTMLFormElement.prototype.requestSubmit = () => again++;
    p.g.reply = () => ({ ...res(''), text: async () => { throw new Error('body cut off'); } });
    p.submit(p.form({}, [['a', '1']]));
    await tick();
    assert.equal(p.fetches.filter((f) => f.method == 'POST').length, 1);
    assert.equal(again, 0);
    // One that never got an answer is the browser's to send.
    const q = page();
    q.g.HTMLFormElement.prototype.requestSubmit = () => again++;
    globalThis.fetch = async () => { throw new TypeError('network'); };
    q.submit(q.form({}, [['a', '1']]));
    await tick();
    assert.equal(again, 1);
  },
  async 'a fragment that is not valid percent-encoding does not break a navigation'() {
    const p = page();
    p.click({}, '/next#%E0%A4%A');
    await tick();
    assert.equal(p.calls.assign, undefined);
    assert.equal(p.loc.href, 'http://x.test/next#%E0%A4%A');
  },
  async 'a page that cannot be shown is loaded whole'() {
    const p = page();
    p.g.reply = () => ({ ...res(''), text: async () => { throw new Error('cut off'); } });
    p.click();
    await tick();
    assert.equal(p.calls.assign, 'http://x.test/next');
  },
  async 'spread: dropping an on* key that was no function leaves the rest working'() {
    const extra = fs.readFileSync(path.join(__dirname, '../src/client/extra.js'), 'utf8').replace(/\r\n/g, '\n');
    const from = extra.indexOf('const held =');
    const code = extra.slice(from, extra.indexOf('\n};\n', extra.indexOf('X.spread =', from)) + 4);
    const set = [];
    const el = { localName: 'div', removeEventListener(t, f) { if (typeof f != 'function') throw new TypeError('not a listener'); }, addEventListener() {}, removeAttribute: (k) => set.push('-' + k) };
    let push;
    const X = {};
    new Function('X', 'watch', 'attr', code)(X, (sc, a, L, f) => (push = f), (x, first, e, k) => set.push(k));
    X.spread(0, 0, el, 0, 0, [0, 0]);
    push({ onclick: 'alert(1)', id: 'a', srcdoc: 'x', ONCLICK: 'y' });
    assert.deepEqual(set, ['id']); // only the safe key
    push({ id: 'a' }); // must not throw on the string under onclick
    assert.deepEqual(set, ['id', 'id']);
  },
  async 'a click navigates and focus moves to the h1'() {
    const p = page();
    const e = p.click();
    assert.ok(e.defaultPrevented);
    await tick();
    assert.equal(p.fetches[0].url, 'http://x.test/next');
    assert.equal(p.loc.href, 'http://x.test/next');
    assert.equal(p.h1.focused, 1);
    assert.deepEqual(p.h1.opts, { preventScroll: true });
    assert.equal(p.h1.attrs.tabindex, '-1');
    p.h1.dispatchEvent(new Event('blur')); // it leaves again
    assert.equal(p.h1.attrs.tabindex, undefined);
    assert.deepEqual(p.calls.scroll, [[0, 0]]);
  },
  async 'data-wisp-keepfocus and data-wisp-noscroll leave focus and scroll'() {
    const p = page();
    p.click({ 'data-wisp-keepfocus': '', 'data-wisp-noscroll': '' });
    await tick();
    assert.equal(p.loc.href, 'http://x.test/next');
    assert.equal(p.h1.focused, 0);
    assert.deepEqual(p.calls.scroll, []);
  },
  async 'beforeNavigate can cancel (wisp:navigate), a pop cannot'() {
    const p = page();
    let seen;
    p.doc.addEventListener('wisp:navigate', (e) => ((seen = e.detail), e.preventDefault()));
    p.click();
    await tick();
    assert.equal(seen.to, 'http://x.test/next');
    assert.equal(p.fetches.length, 0);
    p.g.history.state = {};
    p.event('wisp:goto', { detail: { url: '/back', pop: true } });
    await tick();
    assert.equal(p.fetches.length, 1);
  },
  async 'onNavigate waits, and what it returns runs after the page changed'() {
    const p = page();
    const order = [];
    p.doc.addEventListener('wisp:leave', (e) => e.detail.w.push(new Promise((r) => setTimeout(() => (order.push('wait'), r(() => order.push('after'))), 20))));
    p.doc.addEventListener('wisp:update', () => order.push('update'));
    p.click();
    await new Promise((r) => setTimeout(r, 60));
    assert.deepEqual(order, ['wait', 'update', 'after']);
  },
  async 'view transitions run on a navigation, not under data-wisp-notransition'() {
    const p = page();
    let vts = 0;
    p.doc.startViewTransition = (f) => (vts++, { ready: Promise.resolve(), finished: Promise.resolve(), updateCallbackDone: Promise.resolve(f()) });
    globalThis.matchMedia = () => ({ matches: false });
    p.click();
    await tick();
    assert.equal(vts, 1);
    p.click({ 'data-wisp-notransition': '' });
    await tick();
    assert.equal(vts, 1);
  },
  async 'preloadData fetches the page once, and the click uses it'() {
    const p = page();
    let done = 0;
    p.event('wisp:preload', { detail: { url: '/next', done: () => done++ } });
    p.event('wisp:preload', { detail: { url: '/next', done: () => done++ } });
    await tick();
    assert.equal(done, 2);
    assert.equal(p.fetches.length, 1);
    p.click();
    await tick();
    assert.equal(p.fetches.length, 1);
    p.event('wisp:preload', { detail: { url: 'http://other.test/', done: () => done++ } });
    await tick();
    assert.equal(p.fetches.length, 1); // another site: nothing
  },
  'scripts: idle ones load when idle, interaction ones at the first key'() {
    const script = (src, type) => ({ src, type, attributes: [{ name: 'src', value: src }, { name: 'type', value: type }, { name: 'async', value: '' }] });
    const p = page({ scripts: [script('http://t.test/a.js', 'wisp/idle'), script('http://t.test/b.js', 'wisp/interaction')] });
    assert.equal(p.calls.head.length, 0);
    p.idle.forEach((f) => f());
    assert.equal(p.calls.head.length, 1);
    assert.deepEqual(p.calls.head[0].attrs, { src: 'http://t.test/a.js', async: '' });
    p.win.dispatchEvent(new Event('keydown'));
    p.win.dispatchEvent(new Event('keydown'));
    assert.equal(p.calls.head.length, 2);
    assert.equal(p.calls.head[1].attrs.src, 'http://t.test/b.js');
  },
  async 'loading: the deepest folder view shows in main until the page arrives'() {
    const main = { html: '', nodes: ['page'], attrs: {}, get childNodes() { return this.nodes; }, replaceChildren(...n) { this.html = ''; this.nodes = n; }, insertAdjacentHTML(_, h) { this.html += h; }, setAttribute(k, v) { this.attrs[k] = v; }, removeAttribute(k) { delete this.attrs[k]; } };
    const p = page({ views: [['', '<i>all</i>'], ['/blog', '<i>blog</i>'], ['/shop/[id=int]', '<i>item</i>']], main });
    let release;
    p.g.reply = () => new Promise((r) => (release = () => r(res('<title>B</title><h1>B</h1>'))));
    p.click({}, '/blog/post');
    assert.equal(main.html, '<i>blog</i>');
    assert.equal(main.attrs['aria-busy'], 'true');
    release();
    await tick();
    p.click({}, '/about');
    assert.equal(main.html, '<i>all</i>');
    release();
    await tick();
    main.html = '';
    p.click({}, '/shop/x');
    assert.equal(main.html, '<i>all</i>'); // [id=int] does not fit x
    release();
    await tick();
    main.html = '';
    p.click({}, '/shop/7');
    assert.equal(main.html, '<i>item</i>');
    release();
    await tick();
  },
  async 'loading: a download puts the page back, and navigating ends'() {
    const main = { html: '', nodes: ['page'], attrs: {}, get childNodes() { return this.nodes; }, replaceChildren(...n) { this.html = ''; this.nodes = n; }, insertAdjacentHTML(_, h) { this.html += h; }, setAttribute(k, v) { this.attrs[k] = v; }, removeAttribute(k) { delete this.attrs[k]; } };
    const p = page({ views: [['', '<i>all</i>']], main });
    let stayed = 0;
    p.doc.addEventListener('wisp:stay', () => stayed++);
    p.g.reply = () => res('hi', 'text/plain', 200, 'attachment; filename="a.txt"');
    p.click({}, '/file');
    assert.equal(main.html, '<i>all</i>');
    await tick();
    assert.deepEqual(main.nodes, ['page']);
    assert.equal(main.attrs['aria-busy'], undefined);
    assert.equal(stayed, 1);
  },
  async 'intercept: a navigation shows the slot page in place and changes the address'() {
    const slot = { innerHTML: '', getAttribute: () => JSON.stringify([['/gal/item/[id]', '/gal/@modal/(.)item/[id]']]) };
    const p = page({ cuts: [slot] });
    let stayed = 0;
    p.doc.addEventListener('wisp:stay', () => stayed++);
    p.g.reply = () => res('<b>photo 7</b>');
    p.click({}, '/gal/item/7');
    await tick();
    assert.equal(stayed, 1); // the page stays: navigating ends
    assert.equal(p.fetches.length, 1);
    assert.equal(p.fetches[0].url, '/gal/@modal/(.)item/7');
    assert.equal(slot.innerHTML, '<b>photo 7</b>');
    assert.equal(p.loc.href, 'http://x.test/gal/item/7');
    // Another route, or a page without the slot: an ordinary navigation.
    p.click({}, '/other');
    await tick();
    assert.equal(p.fetches[1].url, 'http://x.test/other');
    const bare = page();
    bare.click({}, '/gal/item/7');
    await tick();
    assert.equal(bare.fetches[0].url, 'http://x.test/gal/item/7');
  },
  async 'a fragment with no element on the page starts at the top'() {
    const p = page();
    p.click({}, '/next#missing');
    await tick();
    assert.deepEqual(p.calls.scroll, [[0, 0]]);
  },
  async 'back to an entry on the same page puts its scroll back'() {
    const p = page();
    p.g.history.state = { k: 'a', x: 5, y: 300 };
    p.win.dispatchEvent(new Event('popstate'));
    await tick();
    assert.deepEqual(p.calls.scroll, [[5, 300]]);
  },
  async 'a download from a form still ends with wisp:result'() {
    const p = page();
    URL.createObjectURL = () => 'blob:x';
    p.g.reply = () => ({ ...res('hi', 'text/plain'), blob: async () => ({ size: 2 }) });
    let got;
    const f = p.form({}, [['a', '1']]);
    f.addEventListener('wisp:result', (e) => (got = e.detail));
    p.submit(f);
    await tick();
    assert.equal(got?.status, 200);
  },
  async 'a slot answer that comes after a newer navigation is dropped'() {
    const slot = { innerHTML: '', getAttribute: () => JSON.stringify([['/gal/item/[id]', '/gal/@modal/(.)item/[id]']]) };
    const p = page({ cuts: [slot] });
    let release;
    p.g.reply = (u) => (u.includes('@modal') ? new Promise((r) => (release = () => r(res('<b>photo</b>')))) : res('<title>Other</title>'));
    p.click({}, '/gal/item/7');
    await tick();
    p.click({}, '/other');
    await tick();
    release();
    await tick();
    assert.equal(slot.innerHTML, '');
    assert.equal(p.loc.href, 'http://x.test/other');
  },
  async 'a superseded navigation has its fetch aborted, a form post does not'() {
    const p = page();
    const sent = [];
    p.g.reply = (u, o) => (sent.push(o), new Promise(() => {}));
    p.click({}, '/first');
    await tick();
    p.click({}, '/second');
    await tick();
    assert.equal(sent[0].signal.aborted, true);
    assert.equal(sent[1].signal.aborted, false);
    const f = p.form({}, [['a', '1']]);
    p.submit(f);
    await tick();
    const post = sent.find((o) => o.method == 'POST');
    p.click({}, '/third');
    await tick();
    assert.equal(post.signal, undefined);
    assert.equal(sent[1].signal.aborted, true);
  },
  async 'a refresh that answers after a navigation is dropped'() {
    const p = page();
    let release;
    p.g.reply = (u) => (u == 'http://x.test/' ? new Promise((r) => (release = () => r(res('<title>Old</title>')))) : res('<title>Next</title>'));
    p.doc.dispatchEvent(new CustomEvent('wisp:refresh'));
    await tick();
    p.click();
    await tick();
    release();
    await tick();
    assert.equal(p.doc.title, 'Next');
  },
  async 'a refresh still lands after a #hash link on the same page'() {
    const p = page();
    let release;
    p.g.reply = (u) => (u == 'http://x.test/' ? new Promise((r) => (release = () => r(res('<title>Fresh</title>')))) : res('<title>Other</title>'));
    p.doc.dispatchEvent(new CustomEvent('wisp:refresh'));
    await tick();
    p.loc.href = 'http://x.test/#part'; // what the browser does for the link
    release();
    await tick();
    assert.equal(p.doc.title, 'Fresh');
  },
  async 'a post that answers after a newer navigation is not shown'() {
    const p = page();
    let release;
    p.g.reply = (u) => (u.endsWith('/save') ? new Promise((r) => (release = () => r(res('<title>Posted</title>')))) : res('<title>Next</title>'));
    p.submit(p.form({}, [['a', '1']]));
    await tick();
    p.click();
    await tick();
    release();
    await tick();
    assert.equal(p.doc.title, 'Next');
  },
  async 'a post drops what was fetched ahead'() {
    const p = page();
    p.event('wisp:preload', { detail: { url: '/next' } });
    await tick();
    p.submit(p.form({}, [['a', '1']]));
    await tick();
    p.click();
    await tick();
    assert.equal(p.fetches.filter((f) => f.url == 'http://x.test/next').length, 2);
  },
  'vitals: only with the meta tag, and one beacon when hidden'() {
    const p = page();
    p.doc.visibilityState = 'hidden';
    p.event('visibilitychange');
    assert.equal(p.calls.beacon.length, 0);
    const q = page({ vitals: { content: '/vitals' } });
    q.event('visibilitychange');
    assert.equal(q.calls.beacon.length, 0);
    q.doc.visibilityState = 'hidden';
    q.event('visibilitychange');
    assert.equal(q.calls.beacon[0][0], '/vitals');
    assert.equal(JSON.parse(q.calls.beacon[0][1]).path, '/');
    // Hidden again with nothing new: not sent twice. A beacon that throws is not an error.
    q.event('visibilitychange');
    assert.equal(q.calls.beacon.length, 1);
    const r = page({ vitals: { content: '/vitals' } });
    r.nav.sendBeacon = () => { throw new Error('no'); };
    r.doc.visibilityState = 'hidden';
    r.event('visibilitychange');
  },
};

// extra.js's motion helpers, with `wisp` and the clock stood in for.
function motion(more = {}) {
  const code = fs.readFileSync(path.join(__dirname, '../src/client/extra.js'), 'utf8').replace(/^import .*$/m, '');
  let now = 0;
  let frames = [];
  const X = { shared: {}, outs: 0, flips: 0, watch: (sc, get, L, f, flags) => f(String(get(L) ?? '')), ...more };
  const store = (v) => {
    const o = { v, subs: [], get value() { return this.v; }, set value(x) { this.v = x; }, subscribe() {} };
    return o;
  };
  const g = {
    matchMedia: () => ({ matches: false }),
    performance: { now: () => now },
    requestAnimationFrame: (f) => frames.push(f),
    cancelAnimationFrame: () => {},
    getComputedStyle: () => ({ opacity: '1' }),
  };
  new Function('X', 'page', 'store', ...Object.keys(g), code)(X, null, store, ...Object.values(g));
  const run = (ms) => {
    for (const end = now + ms; now < end; ) {
      now += 16;
      const f = frames;
      frames = [];
      f.forEach((x) => x(now));
    }
  };
  return { X, run };
}

Object.assign(tests, {
  'tweened runs to its target, spring settles, set resolves'() {
    const { X, run } = motion();
    const t = X.shared.tweened(0, { duration: 160 });
    let done = 0;
    t.set(10).then(() => done++);
    run(80);
    assert.ok(t.value > 0 && t.value < 10);
    run(200);
    assert.equal(t.value, 10);
    return tick().then(() => {
      assert.equal(done, 1);
      const a = X.shared.tweened([0, 0], { duration: 0 });
      a.set([1, 2]);
      assert.deepEqual(a.value, [1, 2]);
      const s = X.shared.spring({ x: 0 });
      s.set({ x: 100 });
      run(5000);
      assert.deepEqual(s.value, { x: 100 });
      s.set({ x: 0 }, { hard: true });
      assert.equal(s.value.x, 0);
    });
  },
  'crossfade pairs a leaver with a newcomer of the same key'() {
    const { X, run } = motion();
    const [send, receive] = X.shared.crossfade({ duration: 100 });
    const el = (left) => ({ style: {}, getBoundingClientRect: () => ({ left, top: 0, width: 10, height: 10 }) });
    const a = el(0);
    const b = el(100);
    const out = send(a, { key: 1 });
    const inn = receive(b, { key: 1 });
    inn.tick(0);
    assert.equal(b.style.transform.startsWith('translate(-100px'), true);
    inn.tick(1);
    assert.equal(b.style.transform, '');
    const c = el(5);
    receive(c, { key: 9 }).tick(0.5);
    assert.equal(c.style.transform, undefined);
    assert.equal(c.style.opacity, 0.5);
  },
});

Object.assign(tests, {
  '{:@html} swaps what lies between its anchor and its end comment'() {
    const { X } = motion();
    // The anchor, the markup (a comment among it), the end (data 'h'), the rest.
    const end = { nodeType: 8, data: 'h' };
    const nodes = [{ nodeType: 1 }, { nodeType: 1, html: 'old' }, { nodeType: 8, data: '' }, end, { nodeType: 3 }];
    const link = () => nodes.forEach((n, i) => ((n.nextSibling = nodes[i + 1] || null), (n.remove = () => (nodes.splice(nodes.indexOf(n), 1), link()))));
    link();
    nodes[0].after = (c) => (nodes.splice(1, 0, { nodeType: 1, html: c }), link());
    const doc = globalThis.document;
    globalThis.document = Object.assign(Object.create(doc), { createElement: () => ({ set innerHTML(v) { this.content = v; } }) });
    X.html(null, null, nodes[0], null, 0, ['html', () => '<b>x</b>']);
    globalThis.document = doc;
    assert.deepEqual(nodes.map((n) => n.html), [undefined, '<b>x</b>', undefined, undefined]);
    assert.equal(nodes[2], end);
  },
});

Object.assign(tests, {
  'a snippet given to a component is drawn after its anchor, with the arguments as parameters'() {
    class Sig { constructor(v) { this.v = v; } }
    const drawn = [];
    const { X } = motion({
      Sig,
      node: (sc, f) => f(),
      scope: (up, c) => ({ up, c }),
      end() {},
      untrack: (f) => f(),
      range: (c) => [c],
      place: (at, el, cs, inst, L, q) => (drawn.push({ at, el, inst, L }), { drawn: drawn.length }),
    });
    const el = {};
    const parent = { who: 'page' };
    X.snip(null, parent, el, { page: 1 }, 0, ['snip', 'row', ['x', 'i']]);
    assert.equal(el.__sn[0], 'row');
    const stops = [];
    let args = ['a', 0];
    X.draw({ stops }, null, 'anchor', {}, 0, ['draw', () => el.__sn[1], () => args]);
    assert.equal(stops.length, 1);
    const d = drawn[0];
    assert.equal(d.at, 'anchor');
    assert.equal(d.inst, parent);
    assert.deepEqual([d.L.page, d.L.x, d.L.i], [1, 'a', 0]);
    // No snippet given: nothing drawn.
    X.draw({ stops }, null, 'anchor', {}, 0, ['draw', () => undefined, () => args]);
    assert.equal(drawn.length, 1);
  },
});

(async () => {
  let failed = 0;
  for (const [name, f] of Object.entries(tests)) {
    try {
      await f();
      console.log('ok   ' + name);
    } catch (e) {
      failed++;
      console.log('FAIL ' + name + '\n' + (e.stack || e));
    }
  }
  process.exit(failed ? 1 : 0);
})();
