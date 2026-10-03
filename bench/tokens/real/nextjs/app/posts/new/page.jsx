// @feature crud
import PostForm from '../form';
import { create } from '../actions';

export const metadata = { title: 'New post' };

export default function NewPost() {
  return <PostForm action={create} />;
}
