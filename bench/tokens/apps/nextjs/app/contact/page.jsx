// @feature form
'use client';
import { useActionState } from 'react';
import { send } from './actions';

export default function Contact() {
  const [state, action] = useActionState(send, {});
  return (
    <form action={action}>
      <title>Contact</title>
      <label>Name <input name="name" required minLength={1} pattern="[\s\S]{0,50}" defaultValue={state.name} />
        {state.errors?.name && <small className="problem">{state.errors.name}</small>}</label>
      <label>Email <input name="email" type="email" required defaultValue={state.email} />
        {state.errors?.email && <small className="problem">{state.errors.email}</small>}</label>
      <button>Send</button>
    </form>
  );
}
