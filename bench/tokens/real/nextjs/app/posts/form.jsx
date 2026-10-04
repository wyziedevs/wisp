// @feature crud
'use client';
import { useActionState } from 'react';

export default function PostForm({ action, post = {} }) {
  const [state, formAction] = useActionState(action, post);
  return (
    <form action={formAction}>
      <label>Title <input name="title" required minLength={1} maxLength={100} defaultValue={state.title} />
        {state.errors?.title && <small className="problem">{state.errors.title}</small>}</label>
      <label>Body <textarea name="body" required defaultValue={state.body} />
        {state.errors?.body && <small className="problem">{state.errors.body}</small>}</label>
      <button>Save</button>
    </form>
  );
}
