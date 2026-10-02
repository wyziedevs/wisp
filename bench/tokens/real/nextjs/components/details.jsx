// @feature component
'use client';
import { useState } from 'react';

export default function Details({ title, children }) {
  const [open, setOpen] = useState(false);
  return (
    <section>
      <button onClick={() => setOpen(!open)}>{title}</button>
      {open && children}
    </section>
  );
}
