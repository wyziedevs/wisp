// @feature crud
import Link from 'next/link';
import { posts } from '@/lib/db';
import Details from '@/components/details';
import { remove } from './actions';
// @feature live
import Live from './live';
// @feature crud

export const metadata = { title: 'Posts' };

export default async function Posts({ searchParams }) {
  const page = Math.max(1, Number((await searchParams).page) || 1);
  const all = [...posts.values()].reverse();
  return (
    <>
      <Link href="/posts/new">New post</Link>
      {all.slice(page * 10 - 10, page * 10).map((post) => (
        <Details key={post.id} title={post.title}>
          <p>{post.body}</p>
          <Link href={`/posts/${post.id}/edit`}>Edit</Link>
          <form action={remove.bind(null, post.id)}>
            <button>Delete</button>
          </form>
        </Details>
      ))}
      {page > 1 && <Link href={`?page=${page - 1}`}>Newer</Link>}
      {all.length > page * 10 && <Link href={`?page=${page + 1}`}>Older</Link>}
      {/* @feature live */}
      <Live />
    </>
  );
}
