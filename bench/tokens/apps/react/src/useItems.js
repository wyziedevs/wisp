// @feature data
import { useEffect, useState } from 'react'

export function useItems() {
  const [items, setItems] = useState([])
  useEffect(() => {
    fetch('/api/items')
      .then((res) => res.json())
      .then(setItems)
  }, [])
  return items
}
