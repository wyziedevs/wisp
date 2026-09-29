// The js-framework-benchmark's operations on /a2/bench's keyed table,
// timed against a vanilla table (the floor every framework is measured
// from): script and layout, the median of runs, in ms. On that page, in
// the browser's console: (await import('/_app/c/lib/bench.js')).run()
import { tick } from 'wisp'

const med = (a) => a.sort((x, y) => x - y)[a.length >> 1]

async function time(f) {
  const s = performance.now()
  f()
  await tick()
  document.body.offsetHeight
  return performance.now() - s
}

const A = ['pretty', 'large', 'big', 'small', 'tall', 'short', 'long', 'handsome', 'plain', 'quaint', 'clean', 'elegant', 'easy', 'angry', 'crazy', 'helpful', 'mushy', 'odd', 'unsightly', 'adorable', 'important', 'inexpensive', 'cheap', 'expensive', 'fancy']
const C = ['red', 'yellow', 'blue', 'green', 'pink', 'brown', 'purple', 'brown', 'white', 'black', 'orange']
const N = ['table', 'chair', 'house', 'bbq', 'desk', 'car', 'pony', 'cookie', 'sandwich', 'burger', 'pizza', 'mouse', 'keyboard']
const pick = (a) => a[Math.round(Math.random() * 1000) % a.length]

// Template cloning and one delegated listener, as the vanillajs entry.
function vanilla() {
  let id = 1
  let data = []
  let rows = []
  let sel = null
  const table = document.createElement('table')
  const tbody = document.createElement('tbody')
  table.append(tbody)
  document.body.append(table)
  const tpl = document.createElement('template')
  tpl.innerHTML = '<tr><td> </td><td><a class="lbl"> </a></td><td><a class="rm">x</a></td></tr>'
  const row0 = tpl.content.firstChild
  const mk = (d) => {
    const r = row0.cloneNode(true)
    r.firstChild.firstChild.nodeValue = d.id
    r.childNodes[1].firstChild.firstChild.nodeValue = d.label
    return r
  }
  const make = (n) => Array.from({ length: n }, () => ({ id: id++, label: `${pick(A)} ${pick(C)} ${pick(N)}` }))
  tbody.onclick = (e) => {
    const a = e.target.closest('a')
    if (!a) return
    const tr = a.closest('tr')
    if (a.className == 'lbl') {
      if (sel) sel.className = ''
      ;(sel = tr).className = 'danger'
    } else {
      const i = rows.indexOf(tr)
      data.splice(i, 1)
      rows.splice(i, 1)
      tr.remove()
    }
  }
  const clear = () => {
    tbody.textContent = ''
    data = []
    rows = []
    sel = null
  }
  const append = (d) => {
    const r = d.map(mk)
    const f = document.createDocumentFragment()
    f.append(...r)
    tbody.append(f)
    data.push(...d)
    rows.push(...r)
  }
  return {
    run: (n) => (clear(), append(make(n))),
    add: () => append(make(1000)),
    update() {
      for (let i = 0; i < data.length; i += 10) {
        data[i].label += ' !!!'
        rows[i].childNodes[1].firstChild.firstChild.nodeValue = data[i].label
      }
    },
    clear,
    swap() {
      if (rows.length < 999) return
      const [a, b] = [rows[1], rows[998]]
      const an = a.nextSibling
      tbody.insertBefore(a, b)
      tbody.insertBefore(b, an)
      ;[rows[1], rows[998]] = [b, a]
      ;[data[1], data[998]] = [data[998], data[1]]
    },
    lbl: () => tbody.querySelectorAll('a.lbl'),
    rm: () => tbody.querySelectorAll('a.rm'),
    done: () => table.remove(),
  }
}

const click = (id) => document.getElementById(id).click()
const wisp = {
  run: (n) => click(n > 1000 ? 'runlots' : 'run'),
  add: () => click('add'),
  update: () => click('update'),
  clear: () => click('clear'),
  swap: () => click('swaprows'),
  lbl: () => document.querySelectorAll('#tbody a.lbl'),
  rm: () => document.querySelectorAll('#tbody a.rm'),
  done() {},
}

const OPS = {
  create1k: async (F) => (await time(F.clear), time(() => F.run(1000))),
  replace1k: async (F) => (await time(() => F.run(1000)), time(() => F.run(1000))),
  update10th: async (F) => (await time(() => F.run(1000)), await time(F.update), time(F.update)),
  select: async (F) => {
    await time(() => F.run(1000))
    const a = F.lbl()
    await time(() => a[5].click())
    return time(() => a[7].click())
  },
  swap: async (F) => (await time(() => F.run(1000)), time(F.swap)),
  remove: async (F) => {
    await time(() => F.run(1000))
    const a = F.rm()
    return time(() => a[500].click())
  },
  create10k: async (F) => (await time(F.clear), time(() => F.run(10000))),
  append1k: async (F) => (await time(() => F.run(1000)), time(F.add)),
  clear1k: async (F) => (await time(() => F.run(1000)), time(F.clear)),
}

// Wisp's times, vanilla's, the ratio, and their geometric mean. The two take
// turns at each operation, so a busy moment of the machine slows both alike.
export async function run() {
  const both = [wisp, vanilla()]
  const out = {}
  let g = 0
  for (const [k, op] of Object.entries(OPS)) {
    const t = [[], []]
    for (let i = 0; i < (k == 'create10k' ? 5 : 11); i++) for (const j of i % 2 ? [1, 0] : [0, 1]) t[j].push(await op(both[j]))
    const [w, v] = t.map((a) => +med(a).toFixed(1))
    const r = Math.max(w, 0.1) / Math.max(v, 0.1)
    out[k] = `${w} / ${v} (${r.toFixed(2)}x)`
    g += Math.log(r)
  }
  for (const F of both) await time(F.clear), F.done()
  out.geomean = Math.exp(g / Object.keys(OPS).length).toFixed(2)
  return out
}
