// Runs live.js (the reactive half of the browser code) against a small DOM
// of its own: nodes, fragments, templates, ranges and a select's options,
// as far as the code under test reaches. Run by client.rs (`node live.js`);
// layout, focus and real events need a browser.
const assert = require('node:assert');
const path = require('node:path');
const { pathToFileURL } = require('node:url');

class N extends EventTarget {
  constructor(t) {
    super();
    this.nodeType = t;
    this.parentNode = null;
    this.kids = [];
  }
  get childNodes() { return this.kids; }
  get firstChild() { return this.kids[0] ?? null; }
  get lastChild() { return this.kids.at(-1) ?? null; }
  get nextSibling() { const p = this.parentNode; return p ? p.kids[p.kids.indexOf(this) + 1] ?? null : null; }
  get children() { return this.kids.filter((k) => k.nodeType == 1); }
  insertBefore(n, ref) {
    if (n.nodeType == 11) {
      for (const k of [...n.kids]) this.insertBefore(k, ref);
      return n;
    }
    n.remove();
    this.kids.splice(ref ? this.kids.indexOf(ref) : this.kids.length, 0, n);
    n.parentNode = this;
    return n;
  }
  append(...ns) { for (const n of ns) this.insertBefore(n, null); }
  after(n) { this.parentNode.insertBefore(n, this.nextSibling); }
  remove() {
    const p = this.parentNode;
    if (p) p.kids.splice(p.kids.indexOf(this), 1), (this.parentNode = null);
  }
  replaceWith(n) { this.parentNode.insertBefore(n, this); this.remove(); }
  cloneNode(deep) {
    const c = this.nodeType == 1 ? el(this.localName) : new N(this.nodeType);
    if (this.nodeType == 1) for (const [k, v] of this.attrs) c.attrs.set(k, v);
    else c.data = this.data;
    if (deep) {
      for (const k of this.kids) c.append(k.cloneNode(true));
      if (this.content) c.content = this.content.cloneNode(true);
    }
    return c;
  }
  get textContent() { return this.nodeType == 1 || this.nodeType == 11 ? this.kids.map((k) => k.textContent).join('') : this.data; }
  set textContent(v) { this.kids.forEach((k) => (k.parentNode = null)); this.kids = []; this.append(txt(v)); }
}
const txt = (data) => Object.assign(new N(3), { data });
const comment = (data) => Object.assign(new N(8), { data });
function el(tag, attrs = {}, ...kids) {
  const e = new N(1);
  e.localName = tag;
  e.nodeName = tag.toUpperCase();
  e.attrs = new Map(Object.entries(attrs));
  e.style = {};
  e.classList = { toggle() {} };
  e.getAttribute = (k) => e.attrs.get(k) ?? null;
  e.hasAttribute = (k) => e.attrs.has(k);
  e.setAttribute = (k, v) => e.attrs.set(k, String(v));
  e.removeAttribute = (k) => e.attrs.delete(k);
  Object.defineProperty(e, 'dataset', { get() { const o = {}; for (const [k, v] of e.attrs) if (k.startsWith('data-')) o[k.slice(5)] = v; return o; } });
  if (tag == 'template') e.content = new N(11);
  if (tag == 'input') {
    e.value = e.attrs.get('value') ?? '';
    e.defaultValue = e.value;
    e.type = e.attrs.get('type') ?? 'text';
  }
  if (tag == 'option') {
    e.selected = false;
    Object.defineProperty(e, 'value', { get: () => e.attrs.get('value') ?? e.textContent });
  }
  if (tag == 'select') {
    const opts = () => { const o = []; const w = (n) => n.kids.forEach((k) => (k.localName == 'option' ? o.push(k) : w(k))); w(e); return o; };
    e.multiple = false;
    Object.defineProperty(e, 'options', { get: opts });
    // As a browser's: no option matches, none is chosen; options put in
    // afterwards leave the first chosen.
    Object.defineProperty(e, 'value', {
      get: () => { const o = opts(); return (o.find((x) => x.selected) ?? o[0])?.value ?? ''; },
      set: (v) => { let hit = false; for (const o of opts()) o.selected = !hit && o.value === v && (hit = true); },
    });
  }
  e.append(...kids.map((k) => (typeof k == 'string' ? txt(k) : k)));
  return e;
}
const tpl = (attrs, ...kids) => { const t = el('template', attrs); t.content.append(...kids); return t; };

const document = new N(9);
Object.assign(document, {
  documentElement: { lang: '' },
  body: el('body'),
  ids: {},
  getElementById(id) { return this.ids[id] ?? null; },
  querySelector: () => null,
  querySelectorAll(s) {
    assert.equal(s, '[data-w]');
    const out = [];
    const w = (n) => n.kids.forEach((k) => (k.nodeType == 1 && (k.attrs.has('data-w') && out.push(k), w(k))));
    w(this.body);
    return out;
  },
  createElement: (t) => el(t),
  createTextNode: txt,
  createComment: comment,
  createDocumentFragment: () => new N(11),
  createRange: () => ({
    setStartBefore(n) { this.s = n; },
    setEndAfter(n) { this.e = n; },
    deleteContents() {
      const p = this.s.parentNode;
      assert.equal(this.e.parentNode, p, 'a range across parents');
      const [a, b] = [p.kids.indexOf(this.s), p.kids.indexOf(this.e)];
      for (const n of p.kids.slice(a, b + 1)) n.remove();
    },
  }),
});
const history = { state: null };
const g = { document, history, location: { href: 'http://x.test/' } };
for (const k in g) Object.defineProperty(globalThis, k, { value: g[k], configurable: true, writable: true });

const tick = () => new Promise((r) => setTimeout(r, 5));
let L;
let n = 0;
// A page of `body` elements, its module `fn(helpers)` giving `{ g: groups, ...state }`; started.
async function run(body, fn) {
  const id = 'm' + n++;
  let st;
  L.define(id, (blob, h) => (st = fn(h)));
  document.body = el('body', {}, ...body);
  document.ids['wisp-live'] = { textContent: JSON.stringify({ m: {}, i: [[0, id, -1, {}]] }) };
  document.dispatchEvent(new CustomEvent('wisp:update', { detail: {} }));
  await tick();
  return st;
}
const li = (...a) => el('li', { 'data-w': '2' }, ...a);
const names = (ul) => ul.children.filter((c) => c.localName == 'li').map((c) => c.textContent).join();
const list = (get, key) => [[], [['each', get, ['item'], key]], [['text', (l) => l.item.name]]];

const tests = {
  async 'each: copies with one key do not outlive their items'() {
    const ul = el('ul', {}, tpl({ 'data-w': '0.1' }, li()));
    const s = await run([ul], (h) => {
      const items = h.__wisp_s([{ id: 1, name: 'a' }, { id: 1, name: 'b' }]);
      return { items, g: list(() => items.v, (l) => l.item.id) };
    });
    assert.equal(names(ul), 'a,b');
    s.items.v = [{ id: 2, name: 'c' }];
    await tick();
    assert.equal(names(ul), 'c');
    s.items.v = [];
    await tick();
    assert.equal(names(ul), '');
  },
  async 'each: reorders, removes and empties by key'() {
    const ul = el('ul', {}, tpl({ 'data-w': '0.1' }, li()));
    const s = await run([ul], (h) => {
      const items = h.__wisp_s([]);
      return { items, g: list(() => items.v, (l) => l.item.id) };
    });
    const set = (...ids) => (s.items.v = ids.map((id) => ({ id, name: 'n' + id })));
    set(1, 2, 3);
    await tick();
    assert.equal(names(ul), 'n1,n2,n3');
    set(3, 1, 4);
    await tick();
    assert.equal(names(ul), 'n3,n1,n4');
    set();
    await tick();
    assert.equal(names(ul), '');
    set(5, 6);
    await tick();
    assert.equal(names(ul), 'n5,n6');
  },
  async 'bind:value: a select whose options are drawn after it keeps the variable'() {
    const sel = el('select', { 'data-w': '0.1' }, tpl({ 'data-w': '0.2' }, el('option', { 'data-w': '3' })));
    const s = await run([sel], (h) => {
      const opts = h.__wisp_s(['a', 'b', 'c']);
      const pick = h.__wisp_s('b');
      return {
        pick,
        g: [[], [['bind', 'value', (l) => pick.v, (l, v) => (pick.v = v)]], [['each', () => opts.v, ['o'], null]], [['text', (l) => l.o]]],
      };
    });
    assert.equal(sel.value, 'b');
    assert.equal(s.pick.v, 'b');
  },
  async 'each: the first draw takes the server\'s copies over'() {
    const paint = (t) => [comment('['), li(t), comment(']')];
    const ul = el('ul', {}, tpl({ 'data-w': '0.1' }, li()), ...paint('a'), ...paint('b'));
    const painted = ul.children.slice(1);
    const s = await run([ul], (h) => {
      const items = h.__wisp_s([{ id: 1, name: 'a' }, { id: 2, name: 'b' }]);
      return { items, g: list(() => items.v, (l) => l.item.id) };
    });
    assert.equal(names(ul), 'a,b');
    assert.deepEqual(ul.children.slice(1), painted); // the same nodes
    s.items.v = [{ id: 2, name: 'B' }, { id: 1, name: 'A' }, { id: 3, name: 'c' }];
    await tick();
    assert.equal(names(ul), 'B,A,c');
    assert.deepEqual(ul.children.slice(1, 3), [painted[1], painted[0]]);
  },
  async 'if: what a removed block read does not run again'() {
    const div = el('div', {}, tpl({ 'data-w': '0.1' }, el('b', { 'data-w': '2' })));
    let reads = 0;
    const s = await run([div], (h) => {
      const on = h.__wisp_s(true);
      const name = h.__wisp_s('a');
      return { on, name, g: [[], [['if', () => on.v, []]], [['text', () => (reads++, name.v)]]] };
    });
    assert.equal(div.textContent, 'a');
    reads = 0;
    s.name.v = 'b';
    s.on.v = false; // one batch: the block goes before its text is drawn again
    await tick();
    assert.equal(reads, 0);
    assert.equal(div.textContent, '');
    s.name.v = 'c';
    await tick();
    assert.equal(reads, 0);
    s.on.v = true;
    await tick();
    assert.equal(div.textContent, 'c');
  },
  async 'store: update() in place tells its subscribers'() {
    const s = L.store({ n: 1 });
    const seen = [];
    s.subscribe((v) => seen.push(v.n));
    s.update((v) => ((v.n = 2), v));
    await tick();
    assert.deepEqual(seen, [1, 2]);
  },
  async 'bind:value: a form reset (after its listeners, as a browser does it) moves the variable'() {
    const inp = el('input', { 'data-w': '0.1' });
    inp.form = el('form', {}, inp);
    const s = await run([inp.form], (h) => {
      const v = h.__wisp_s('x');
      return { v, g: [[], [['bind', 'value', () => v.v, (l, x) => (v.v = x)]]] };
    });
    assert.equal(inp.value, 'x');
    inp.value = 'y';
    inp.dispatchEvent(new Event('input'));
    assert.equal(s.v.v, 'y');
    // A browser runs the microtasks between the reset event and the reset itself.
    setTimeout(() => (inp.value = inp.defaultValue));
    inp.form.dispatchEvent(new Event('reset'));
    await tick();
    assert.equal(s.v.v, '');
  },
  async 'start: a morph keeps the instance and its copies, or replaces them whole'() {
    const ul = el('ul', {}, tpl({ 'data-w': '0.1' }, li()));
    const s = await run([ul], (h) => {
      const items = h.__wisp_s([{ id: 1, name: 'a' }, { id: 2, name: 'b' }]);
      return { items, g: list(() => items.v, (l) => l.item.id) };
    });
    assert.equal(names(ul), 'a,b');
    const again = () => (document.dispatchEvent(new CustomEvent('wisp:update', { detail: {} })), tick());
    await again(); // the same elements: kept
    s.items.v = [{ id: 2, name: 'b' }, { id: 3, name: 'c' }];
    await tick();
    assert.equal(names(ul), 'b,c');
    // The elements replaced (the page's own copies stay put, as a morph leaves them).
    const ul2 = el('ul', {}, tpl({ 'data-w': '0.1' }, li()));
    document.body.kids[0].replaceWith(ul2);
    await again();
    assert.equal(names(ul2), 'b,c');
    s.items.v = [{ id: 4, name: 'd' }];
    await tick();
    assert.equal(names(ul2), 'd');
  },
};

(async () => {
  L = await import(pathToFileURL(path.join(__dirname, '../src/client/live.js')).href);
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
