// @feature upload
'use client';
import { useActionState } from 'react';
import { upload } from './actions';

export default function AvatarForm() {
  const [state, action] = useActionState(upload, {});
  return (
    <form action={action}>
      <label>Avatar <input name="avatar" type="file" required accept="image/*" />
        {state.error && <small className="problem">{state.error}</small>}</label>
      <button>Upload</button>
    </form>
  );
}
