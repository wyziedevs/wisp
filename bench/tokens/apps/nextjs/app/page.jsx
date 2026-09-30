// @feature list
import { items } from '@/lib/db';

export const metadata = { title: 'Items' };

export default async function Page() {
  const list = await items();
  return (
    <ul>
      {list.map((item) => (
        <li key={item.id}>
          {item.name}: ${item.price}
        </li>
      ))}
    </ul>
  );
}
