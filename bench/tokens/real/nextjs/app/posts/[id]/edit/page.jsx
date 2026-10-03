// @feature crud
import { notFound } from 'next/navigation';
import { posts } from '@/lib/db';
import PostForm from '../../form';
import { update } from '../../actions';

export default async function EditPost({ params }) {
  const post = posts.get(Number((await params).id));
  if (!post) notFound();
  return (
    <>
      <title>{`Edit ${post.title}`}</title>
      <PostForm action={update.bind(null, post.id)} post={post} />
    </>
  );
}
