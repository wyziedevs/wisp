// The golden live page under Node: wisp.js and live.js, as the app serves
// them, run against tests/golden.html in a DOM just big enough for them.
// Run by golden.rs with the directory it wrote the scripts to; prints
// `ok`, or throws.
import { readFileSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { pathToFileURL } from 'node:url';
import vm from 'node:vm';

const dir = process.argv[2];
const read = (f) => readFileSync(join(dir, f), 'utf8');

// ---- a small DOM ------------------------------------------------------------

class Node {
  constructor(type, name) {
    this.nodeType = type;
    this.nodeName = name;
    this.parentNode = null;
    this.childNodes = [];
  }
  get parentElement() {
    return this.parentNode?.nodeType == 1 ? this.parentNode : null;
  }
  get firstChild() {
    return this.childNodes[0] || null;
  }
  get lastChild() {
    return this.childNodes.at(-1) || null;
  }
  get nextSibling() {
    const s = this.parentNode?.childNodes;
    return (s && s[s.indexOf(this) + 1]) || null;
  }
  get previousSibling() {
    const s = this.parentNode?.childNodes;
    return (s && s[s.indexOf(this) - 1]) || null;
  }
  get textContent() {
    return this.nodeType == 3 || this.nodeType == 8 ? this.data : this.childNodes.map((c) => (c.nodeType == 8 ? '' : c.textContent)).join('');
  }
  set textContent(v) {
    if (this.nodeType == 3 || this.nodeType == 8) return void (this.data = String(v));
    for (const c of this.childNodes) c.parentNode = null;
    this.childNodes = [];
    if (v !== '') this.appendChild(document.createTextNode(String(v)));
  }
  get nodeValue() {
    return this.nodeType == 3 || this.nodeType == 8 ? this.data : null;
  }
  set nodeValue(v) {
    if (this.nodeType == 3 || this.nodeType == 8) this.data = String(v);
  }
  get isConnected() {
    let n = this;
    while (n.parentNode) n = n.parentNode;
    return n === document;
  }
  insertBefore(n, ref) {
    if (n.nodeType == 11) {
      for (const c of [...n.childNodes]) this.insertBefore(c, ref);
      return n;
    }
    n.parentNode?.removeChild(n);
    const at = ref ? this.childNodes.indexOf(ref) : this.childNodes.length;
    if (at < 0) throw new Error('insertBefore: not a child');
    this.childNodes.splice(at, 0, n);
    n.parentNode = this;
    return n;
  }
  appendChild(n) {
    return this.insertBefore(n, null);
  }
  append(...ns) {
    for (const n of ns) this.appendChild(typeof n == 'string' ? document.createTextNode(n) : n);
  }
  removeChild(n) {
    const at = this.childNodes.indexOf(n);
    if (at < 0) throw new Error('removeChild: not a child');
    this.childNodes.splice(at, 1);
    n.parentNode = null;
    return n;
  }
  remove() {
    this.parentNode?.removeChild(this);
  }
  before(...ns) {
    for (const n of ns) this.parentNode.insertBefore(n, this);
  }
  after(...ns) {
    const next = this.nextSibling;
    for (const n of ns) this.parentNode.insertBefore(n, next);
  }
  replaceWith(...ns) {
    this.before(...ns);
    this.remove();
  }
  contains(n) {
    for (; n; n = n.parentNode) if (n === this) return true;
    return false;
  }
  cloneNode(deep) {
    let c;
    if (this.nodeType == 3) c = document.createTextNode(this.data);
    else if (this.nodeType == 8) c = document.createComment(this.data);
    else if (this.nodeType == 11) c = document.createDocumentFragment();
    else {
      c = document.createElement(this.localName);
      for (const [k, v] of this.attrs) c.attrs.set(k, v);
      if (this.content) c.content = this.content.cloneNode(true);
    }
    if (deep) for (const k of this.childNodes) c.appendChild(k.cloneNode(true));
    return c;
  }
  // Events: capture is not told apart; enough for a click.
  addEventListener(type, f, o) {
    ((this.on ||= {})[type] ||= []).push([f, typeof o == 'object' && o?.once]);
    o?.signal?.addEventListener('abort', () => this.removeEventListener(type, f));
  }
  removeEventListener(type, f) {
    const l = this.on?.[type];
    if (l) this.on[type] = l.filter((x) => x[0] !== f);
  }
  dispatchEvent(e) {
    const path = [];
    for (let n = this; n; n = n.parentNode) path.push(n);
    if (e.bubbles && this.nodeType != 9 && path.at(-1) === document) path.push(window);
    Object.defineProperty(e, 'target', { value: this, configurable: true });
    e.composedPath = () => path;
    for (const n of e.bubbles ? path : [this]) {
      Object.defineProperty(e, 'currentTarget', { value: n, configurable: true });
      for (const [f, once] of [...(n.on?.[e.type] || [])]) {
        if (once) n.removeEventListener(e.type, f);
        f.call(n, e);
        if (e.stopped) return !e.defaultPrevented;
      }
      if (e.halted) break;
    }
    return !e.defaultPrevented;
  }
  // Selectors: comma lists of `tag`, `#id`, `.class`, `[a]`, `[a="v"]`,
  // `[a^="v"]` together, no combinators.
  querySelectorAll(sel) {
    const tests = sel.split(',').map(compile);
    const out = [];
    const walk = (n) => {
      for (const c of n.childNodes) {
        if (c.nodeType != 1) continue;
        if (tests.some((t) => t(c))) out.push(c);
        walk(c);
      }
    };
    walk(this);
    return out;
  }
  querySelector(sel) {
    return this.querySelectorAll(sel)[0] || null;
  }
  getElementById(id) {
    return this.querySelector(`#${id}`);
  }
}

function compile(sel) {
  const parts = sel.trim().match(/^[\w-]+|#[\w-]+|\.[\w-]+|\[[^\]]+\]/g);
  if (!parts || parts.join('') != sel.trim()) throw new Error(`selector not supported here: ${sel}`);
  const tests = parts.map((p) => {
    if (p[0] == '#') return (el) => el.getAttribute('id') == p.slice(1);
    if (p[0] == '.') return (el) => el.classList.contains(p.slice(1));
    if (p[0] != '[') return (el) => el.localName == p;
    const [, a, op, v] = p.match(/^\[([\w-]+)(?:(\^?=)"?([^"]*)"?)?\]$/);
    if (!op) return (el) => el.hasAttribute(a);
    if (op == '=') return (el) => el.getAttribute(a) == v;
    return (el) => el.getAttribute(a)?.startsWith(v);
  });
  return (el) => tests.every((t) => t(el));
}

class Element extends Node {
  constructor(name) {
    super(1, name.toUpperCase());
    this.localName = name;
    this.tagName = this.nodeName;
    this.attrs = new Map();
    this.style = { setProperty() {}, removeProperty() {} };
    if (name == 'template') this.content = new Node(11, '#document-fragment');
  }
  get attributes() {
    return [...this.attrs].map(([name, value]) => ({ name, value }));
  }
  getAttribute(a) {
    return this.attrs.has(a) ? this.attrs.get(a) : null;
  }
  setAttribute(a, v) {
    this.attrs.set(a, String(v));
  }
  hasAttribute(a) {
    return this.attrs.has(a);
  }
  removeAttribute(a) {
    this.attrs.delete(a);
  }
  get id() {
    return this.getAttribute('id') || '';
  }
  get children() {
    return this.childNodes.filter((c) => c.nodeType == 1);
  }
  get dataset() {
    const d = {};
    for (const [k, v] of this.attrs) if (k.startsWith('data-')) d[k.slice(5).replace(/-(\w)/g, (_, c) => c.toUpperCase())] = v;
    return d;
  }
  get classList() {
    const el = this;
    const list = () => (el.getAttribute('class') || '').split(/\s+/).filter(Boolean);
    const put = (l) => el.setAttribute('class', l.join(' '));
    return {
      contains: (c) => list().includes(c),
      add: (...cs) => put([...new Set([...list(), ...cs])]),
      remove: (...cs) => put(list().filter((c) => !cs.includes(c))),
      toggle: (c, on = !list().includes(c)) => (on ? put([...new Set([...list(), c])]) : put(list().filter((x) => x != c)), on),
    };
  }
  closest(sel) {
    const t = compile(sel);
    for (let n = this; n && n.nodeType == 1; n = n.parentNode) if (t(n)) return n;
    return null;
  }
  matches(sel) {
    return compile(sel)(this);
  }
  click() {
    this.dispatchEvent(new MouseEvent('click', { bubbles: true, cancelable: true }));
  }
}

// ---- the page ---------------------------------------------------------------

const VOID = new Set(['meta', 'link', 'input', 'br', 'img', 'hr', 'source', 'area', 'base', 'col', 'embed', 'wbr']);
const RAW = new Set(['script', 'style']);
const decode = (s) =>
  s.replace(/&(quot|amp|lt|gt|#39);/g, (_, e) => ({ quot: '"', amp: '&', lt: '<', gt: '>', '#39': "'" })[e]);

// Enough of HTML for the pages Wisp writes: no implied end tags.
function parse(html, root) {
  const stack = [root];
  const top = () => {
    const t = stack.at(-1);
    return t.content || t;
  };
  let i = 0;
  while (i < html.length) {
    if (html.startsWith('<!--', i)) {
      const end = html.indexOf('-->', i + 4);
      top().appendChild(document.createComment(html.slice(i + 4, end)));
      i = end + 3;
    } else if (html.startsWith('<!', i)) {
      i = html.indexOf('>', i) + 1;
    } else if (html.startsWith('</', i)) {
      const end = html.indexOf('>', i);
      const name = html.slice(i + 2, end).trim().toLowerCase();
      if (stack.at(-1).localName != name) throw new Error(`</${name}> closes <${stack.at(-1).localName}>`);
      stack.pop();
      i = end + 1;
    } else if (html[i] == '<') {
      const m = /^<([\w-]+)((?:\s+[\w:-]+(?:="[^"]*")?)*)\s*\/?>/.exec(html.slice(i));
      if (!m) throw new Error(`tag at ${i}: ${html.slice(i, i + 40)}`);
      const el = document.createElement(m[1].toLowerCase());
      for (const [, a, v] of m[2].matchAll(/([\w:-]+)(?:="([^"]*)")?/g)) el.attrs.set(a, decode(v ?? ''));
      top().appendChild(el);
      i += m[0].length;
      if (RAW.has(el.localName)) {
        const end = html.indexOf(`</${el.localName}>`, i);
        if (end > i) el.appendChild(document.createTextNode(html.slice(i, end)));
        i = end + el.localName.length + 3;
      } else if (!VOID.has(el.localName)) stack.push(el);
    } else {
      const end = html.indexOf('<', i);
      const to = end < 0 ? html.length : end;
      top().appendChild(document.createTextNode(decode(html.slice(i, to))));
      i = to;
    }
  }
}

class Doc extends Node {
  constructor() {
    super(9, '#document');
  }
  createElement(n) {
    return new Element(n.toLowerCase());
  }
  createTextNode(s) {
    const t = new Node(3, '#text');
    t.data = String(s);
    return t;
  }
  createComment(s) {
    const c = new Node(8, '#comment');
    c.data = String(s);
    return c;
  }
  createDocumentFragment() {
    return new Node(11, '#document-fragment');
  }
  get documentElement() {
    return this.children[0];
  }
  get children() {
    return this.childNodes.filter((c) => c.nodeType == 1);
  }
  get head() {
    return this.querySelector('head');
  }
  get body() {
    return this.querySelector('body');
  }
  get scripts() {
    return this.querySelectorAll('script');
  }
}

class Event {
  constructor(type, o = {}) {
    this.type = type;
    this.bubbles = !!o.bubbles;
    this.cancelable = !!o.cancelable;
    this.detail = o.detail;
    this.defaultPrevented = false;
  }
  preventDefault() {
    if (this.cancelable) this.defaultPrevented = true;
  }
  stopPropagation() {
    this.halted = true;
  }
  stopImmediatePropagation() {
    this.halted = this.stopped = true;
  }
}
class CustomEvent extends Event {}
class MouseEvent extends Event {}

const document = new Doc();
const window = globalThis;
const html = read('golden.html');
const page = 'http://localhost/golden';
parse(html, document);

const events = new Node(0, '#window');
Object.assign(globalThis, {
  document,
  window,
  Event,
  CustomEvent,
  MouseEvent,
  Node,
  Element,
  HTMLElement: Element,
  HTMLAnchorElement: class extends Element {},
  HTMLFormElement: class extends Element {},
  location: new URL(page),
  history: { state: null, scrollRestoration: 'auto', pushState() {}, replaceState() {} },
  addEventListener: (t, f, o) => events.addEventListener(t, f, o),
  removeEventListener: (t, f) => events.removeEventListener(t, f),
  scrollTo() {},
  matchMedia: () => ({ matches: false, addEventListener() {} }),
  requestAnimationFrame: (f) => setTimeout(f, 0),
  requestIdleCallback: (f) => setTimeout(f, 0),
  IntersectionObserver: class {
    observe() {}
    disconnect() {}
  },
  fetch: () => Promise.reject(new Error('no network in this test')),
});
document.currentScript = document.querySelector('script[src]');
document.currentScript.src = new URL(document.currentScript.getAttribute('src'), page).href;

// ---- run --------------------------------------------------------------------

const errors = [];
const log = console.error;
console.error = (...a) => (errors.push(a.join(' ')), log(...a));

// wisp.js, a classic script: parses the instance list once, for live.js.
vm.runInThisContext(read('wisp.js'), { filename: 'wisp.js' });
const list = document.getElementById('wisp-live');
const j = list.__j;
if (!j || !Array.isArray(j.i) || !j.i.length) throw new Error('wisp.js did not read the instance list');
// A record: [I, module, parent, blob, how?].
const [I, id, P, blob, how] = j.i[0];
if (I !== 0 || P !== -1 || how !== undefined || blob.start !== 2 || !j.m[id]) throw new Error(`record: ${JSON.stringify(j)}`);
document.currentScript = null;

// The module, its import of the runtime pointed at the file here; the
// list's URL for it too.
const live = pathToFileURL(join(dir, 'live.mjs')).href;
const mod = join(dir, 'module.mjs');
writeFileSync(mod, read('module.js').replace(/from "\/_app\/live\.js[^"]*"/, `from ${JSON.stringify(live)}`));
j.m[id] = pathToFileURL(mod).href;
await import(live);
for (let k = 0; k < 20 && document.querySelector('button').__on === undefined; k++) await new Promise((r) => setTimeout(r, 5));

// Hydrated: the server's nodes taken over, each Rust loop's values read.
const button = document.querySelector('button');
const lis = document.querySelectorAll('li').map((li) => li.textContent);
const bs = document.querySelectorAll('b').filter((b) => b.isConnected && !b.parentNode.content);
const want = (what, got, ok) => {
  if (!ok) throw new Error(`${what}: ${JSON.stringify(got)}\n${errors.join('\n')}`);
};
want('button', button.textContent, button.textContent == '2');
want('li', lis, lis.join() == 'A,B');
want('painted copies', bs.map((b) => b.textContent), bs.map((b) => b.textContent).join() == 'a,b');
const p = document.querySelector('p');
want('copies taken over, not drawn again', p.childNodes.length, p.querySelectorAll('b').length == 2);

// A click through the root's listener (the `on:` bits) reaches the state.
button.click();
await Promise.resolve();
await new Promise((r) => setTimeout(r, 0));
want('after a click', button.textContent, button.textContent == '3');
want('no errors', errors, !errors.length);
console.log('ok');
