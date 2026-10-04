// What `esbuild src/widget.js --bundle --format=esm --outfile=static/widget.js` writes.
export const mount = (el, props) => {
  const b = document.createElement('b')
  el.append(b)
  const update = (p) => (b.textContent = p.label)
  update(props)
  return { update, destroy: () => b.remove() }
}
