// @feature auth
'use client';
import { useActionState } from 'react';
import { signup } from '../actions';

export default function Signup() {
  const [state, action] = useActionState(signup, {});
  return (
    <form action={action}>
      <title>Sign up</title>
      <input name="email" type="email" defaultValue={state.email} />
      {state.errors?.email && <p>{state.errors.email}</p>}
      <input name="password" type="password" />
      {state.errors?.password && <p>{state.errors.password}</p>}
      <button>Sign up</button>
    </form>
  );
}
