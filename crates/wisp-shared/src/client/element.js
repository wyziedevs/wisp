// element.js: a component as a custom element (`{@element "x-card"}`),
// served as /_app/c/el.js to the modules of /_app/c/el/*.js, so only an
// app with one pays for it. The element works on any page, Wisp's or not:
// it draws its component in its shadow root, with its <style>; its props are
// attributes (read as their Rust types) and properties; what it holds shows
// in its <slot>.
import { __wisp as X } from 'wisp';

const { defs, instance, script, place, range, end, untrack, report } = X;

// An attribute's text as its prop's kind: n a number, b a bool ("false"
// is false), j JSON (else the text), s the text.
function read(kind, s) {
  if (kind == 'n') return +s;
  if (kind == 'b') return s != 'false';
  if (kind == 'j')
    try {
      return JSON.parse(s);
    } catch {}
  return s;
}

// props: { name: [kind, default] }.
export function element(tag, id, props, css) {
  if (customElements.get(tag)) return;
  const names = Object.keys(props);
  const attrs = names.map((n) => n.replace(/_/g, '-'));
  class El extends HTMLElement {
    static get observedAttributes() {
      return attrs;
    }
    constructor() {
      super();
      this.v = {};
      for (const n of names) this.v[n] = props[n][1];
      // A property set before the element was defined.
      for (const n of names)
        if (Object.hasOwn(this, n)) {
          const x = this[n];
          delete this[n];
          this.v[n] = x;
        }
    }
    connectedCallback() {
      if (this.i) return;
      const def = defs.get(id);
      const root = this.shadowRoot || this.attachShadow({ mode: 'open' });
      // Its children show where the component renders them.
      def.el ||= Object.assign(document.createElement('template'), {
        innerHTML: (def.html || '').replaceAll('<template data-wslot></template>', '<slot></slot>'),
      });
      root.textContent = '';
      if (css) root.append(Object.assign(document.createElement('style'), { textContent: css }));
      const at = root.appendChild(document.createTextNode(''));
      const i = (this.i = instance(def, id, at, null, 0));
      i.sc.c = 1;
      this.c = untrack(() => {
        try {
          script(i, { ...this.v });
        } catch (e) {
          report(i.sc, e);
        }
        return place(at, def.el, i.sc, i, {}, true);
      });
    }
    // Moved, it is taken out and put back at once: it keeps its state.
    disconnectedCallback() {
      queueMicrotask(() => {
        if (this.isConnected || !this.i) return;
        end(this.i.sc);
        range(this.c).forEach((n) => n.remove());
        this.i = null;
      });
    }
    attributeChangedCallback(a, was, s) {
      const n = names[attrs.indexOf(a)];
      this.put(n, s == null ? props[n][1] : read(props[n][0], s));
    }
    put(n, x) {
      this.v[n] = x;
      this.i?.s?.({ ...this.v });
    }
  }
  for (const n of names)
    Object.defineProperty(El.prototype, n, {
      get() {
        return this.v[n];
      },
      set(x) {
        this.put(n, x);
      },
    });
  customElements.define(tag, El);
}
