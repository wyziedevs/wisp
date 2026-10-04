// @feature search
import { useState } from 'react'
import { useItems } from '../useItems.js'

export default function Search() {
  const items = useItems()
  const [q, setQ] = useState('')
  const found = items.filter((i) => i.name.toLowerCase().includes(q.toLowerCase()))
  return (
    <>
      <title>Search</title>
      <input value={q} onChange={(e) => setQ(e.target.value)} placeholder="Search" />
      <ul>
        {found.map((item) => (
          <li key={item.id}>{item.name}</li>
        ))}
      </ul>
    </>
  )
}
