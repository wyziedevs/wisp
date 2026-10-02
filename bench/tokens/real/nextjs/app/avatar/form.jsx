// @feature upload
'use client';
import { useActionState } from 'react';
import { upload } from './actions';

export default function AvatarForm() {
  const [state, action] = useActionState(upload, {});
  return (
    <form action={action}>
      <input name="avatar" type="file" accept="image/*" />
      {state.error && <p>{state.error}</p>}
      <button>Upload</button>
    </form>
  );
}
