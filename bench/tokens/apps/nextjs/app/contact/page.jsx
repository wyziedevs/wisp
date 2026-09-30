// @feature form
'use client';
import { useActionState } from 'react';
import { send } from './actions';

export default function Contact() {
  const [state, action] = useActionState(send, {});
  return (
    <form action={action}>
      <title>Contact</title>
      <input name="name" defaultValue={state.name} />
      {state.errors?.name && <p>{state.errors.name}</p>}
      <input name="email" defaultValue={state.email} />
      {state.errors?.email && <p>{state.errors.email}</p>}
      <button>Send</button>
    </form>
  );
}
