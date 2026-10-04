// @feature list
import { useItems } from '../useItems.js'

export default function Items() {
  const items = useItems()
  return (
    <>
      <title>Items</title>
      <ul>
        {items.map((item) => (
          <li key={item.id}>
            {item.name}: ${item.price}
          </li>
        ))}
      </ul>
    </>
  )
}
