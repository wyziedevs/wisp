// @feature form
'use server';
import { redirect } from 'next/navigation';

export async function send(prev, data) {
  const form = Object.fromEntries(data);
  const errors = {};
  if (!form.name || form.name.length > 50) errors.name = 'must have 1 to 50 characters';
  if (!/^\S+@\S+\.\S+$/.test(form.email)) errors.email = 'must be an email address';
  if (Object.keys(errors).length) return { ...form, errors };
  console.log(`${form.name} <${form.email}>`);
  redirect('/');
}
