// @feature form
'use server';
import { redirect } from 'next/navigation';

const EMAIL = /^[^\s@]+@[^\s@]+\.[^\s@]+$/;

export async function send(prev, form) {
  const name = String(form.get('name') ?? '');
  const email = String(form.get('email') ?? '');
  const errors = {};
  if (name.length < 1 || name.length > 50) errors.name = 'Name must be 1 to 50 characters';
  if (!EMAIL.test(email)) errors.email = 'Enter a valid email';
  if (errors.name || errors.email) return { name, email, errors };
  console.log(`${name} <${email}>`);
  redirect('/');
}
