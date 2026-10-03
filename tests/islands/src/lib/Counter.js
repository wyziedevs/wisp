// A React component with a hook: it works only when react-switch, react-dom
// and this file share one copy of React.
import { createElement as h, useState } from 'react'

export function Counter({ start = 0, label }) {
  const [n, set] = useState(start)
  return h('button', { onClick: () => set(n + 1) }, `${label}: ${n}`)
}
