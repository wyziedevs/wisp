// @feature upload
'use server';
import { redirect } from 'next/navigation';
import { currentUser } from '@/lib/auth';

export async function upload(prev, form) {
  const user = await currentUser();
  if (!user) redirect('/login');
  const file = form.get('avatar');
  if (!file?.type?.startsWith('image/')) return { error: 'Choose an image' };
  if (file.size > 1024 * 1024) return { error: 'At most 1 MB' };
  user.avatar = { type: file.type, bytes: new Uint8Array(await file.arrayBuffer()) };
  redirect('/dashboard');
}
