// @feature crud
'use server';
import { revalidatePath } from 'next/cache';
import { notFound, redirect } from 'next/navigation';
import { posts, nextId, checkPost, listeners } from '@/lib/db';

export async function create(prev, form) {
  const { title, body } = Object.fromEntries(form);
  const errors = checkPost({ title, body });
  if (errors) return { title, body, errors };
  const id = nextId();
  posts.set(id, { id, title, body });
  // @feature live
  for (const send of listeners) send();
  // @feature crud
  redirect('/posts');
}

export async function update(id, prev, form) {
  const post = posts.get(id);
  if (!post) notFound();
  const { title, body } = Object.fromEntries(form);
  const errors = checkPost({ title, body });
  if (errors) return { title, body, errors };
  Object.assign(post, { title, body });
  redirect('/posts');
}

export async function remove(id) {
  posts.delete(id);
  revalidatePath('/posts');
}
