// @feature crud
'use client';
import { useActionState } from 'react';

export default function PostForm({ action, post = {} }) {
  const [state, formAction] = useActionState(action, post);
  return (
    <form action={formAction}>
      <input name="title" defaultValue={state.title} />
      {state.errors?.title && <p>{state.errors.title}</p>}
      <textarea name="body" defaultValue={state.body} />
      {state.errors?.body && <p>{state.errors.body}</p>}
      <button>Save</button>
    </form>
  );
}
