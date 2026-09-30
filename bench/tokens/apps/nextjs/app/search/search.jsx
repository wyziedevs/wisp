// @feature search
'use client';
import { useState } from 'react';

export default function Search({ items }) {
  const [q, setQ] = useState('');
  const found = items.filter((i) => i.name.toLowerCase().includes(q.toLowerCase()));
  return (
    <>
      <input value={q} onChange={(e) => setQ(e.target.value)} placeholder="Search" />
      <ul>
        {found.map((item) => (
          <li key={item.id}>{item.name}</li>
        ))}
      </ul>
    </>
  );
}
