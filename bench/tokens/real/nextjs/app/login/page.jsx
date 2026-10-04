// @feature auth
'use client';
import { useActionState } from 'react';
import { login } from '../actions';

export default function Login() {
  const [state, action] = useActionState(login, {});
  return (
    <form action={action}>
      <title>Log in</title>
      <label>Email <input name="email" type="email" required defaultValue={state.email} />
        {state.error && <small className="problem">{state.error}</small>}</label>
      <label>Password <input name="password" type="password" /></label>
      <button>Log in</button>
    </form>
  );
}
