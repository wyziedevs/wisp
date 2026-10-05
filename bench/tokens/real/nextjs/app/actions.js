// @feature auth
'use server';
import { hash, verify } from '@node-rs/argon2';
import { cookies } from 'next/headers';
import { redirect } from 'next/navigation';
import { users, sessions } from '@/lib/db';
import { startSession } from '@/lib/auth';

export async function signup(prev, form) {
  const { email, password } = Object.fromEntries(form);
  const errors = {};
  if (!/^[a-zA-Z0-9.!#$%&'*+\/=?^_`{|}~-]+@[a-zA-Z0-9](?:[a-zA-Z0-9-]{0,61}[a-zA-Z0-9])?(?:\.[a-zA-Z0-9](?:[a-zA-Z0-9-]{0,61}[a-zA-Z0-9])?)*$/.test(email)) errors.email = 'Enter a valid email';
  else if (users.some((u) => u.email === email)) errors.email = 'Already signed up';
  if (password.length < 8) errors.password = 'At least 8 characters';
  if (errors.email || errors.password) return { email, errors };
  const user = { id: users.length + 1, email, hash: await hash(password) };
  users.push(user);
  await startSession(user);
  redirect('/dashboard');
}

export async function login(prev, form) {
  const { email, password } = Object.fromEntries(form);
  const user = users.find((u) => u.email === email);
  if (!user || !(await verify(user.hash, password))) {
    return { email, error: 'Wrong email or password' };
  }
  await startSession(user);
  redirect('/dashboard');
}

export async function logout() {
  const jar = await cookies();
  sessions.delete(jar.get('session')?.value);
  jar.delete('session');
  redirect('/login');
}
