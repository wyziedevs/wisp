// @feature auth
'use client';
import { useActionState } from 'react';
import { login } from '../actions';

export default function Login() {
  const [state, action] = useActionState(login, {});
  return (
    <form action={action}>
      <title>Log in</title>
      <input name="email" type="email" defaultValue={state.email} />
      <input name="password" type="password" />
      {state.error && <p>{state.error}</p>}
      <button>Log in</button>
    </form>
  );
}
