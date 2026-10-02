// @feature auth
import { cookies } from 'next/headers';
import { sessions } from './db';

export async function currentUser() {
  return sessions.get((await cookies()).get('session')?.value);
}

export async function startSession(user) {
  const id = crypto.randomUUID();
  sessions.set(id, user);
  (await cookies()).set('session', id, { httpOnly: true });
}
