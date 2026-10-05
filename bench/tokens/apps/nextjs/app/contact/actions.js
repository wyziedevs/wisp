// @feature form
'use server';
import { redirect } from 'next/navigation';

export async function send(prev, data) {
  const form = Object.fromEntries(data);
  if (typeof form.name != 'string' || typeof form.email != 'string') throw new Error('missing form field');
  const errors = {};
  const n = [...form.name].length;
  if (n < 1) errors.name = 'must have at least 1 character';
  if (n > 50) errors.name = 'must have at most 50 characters';
  if (!/^[a-zA-Z0-9.!#$%&'*+\/=?^_`{|}~-]+@[a-zA-Z0-9](?:[a-zA-Z0-9-]{0,61}[a-zA-Z0-9])?(?:\.[a-zA-Z0-9](?:[a-zA-Z0-9-]{0,61}[a-zA-Z0-9])?)*$/.test(form.email)) errors.email = 'must be an email address';
  if (Object.keys(errors).length) return { ...form, errors };
  console.log(`${form.name} <${form.email}>`);
  redirect('/');
}
