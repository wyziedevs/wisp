// @feature auth
'use client';
import { useActionState } from 'react';
import { signup } from '../actions';

export default function Signup() {
  const [state, action] = useActionState(signup, {});
  return (
    <form action={action}>
      <title>Sign up</title>
      <label>Email <input name="email" type="email" required defaultValue={state.email} />
        {state.errors?.email && <small className="problem">{state.errors.email}</small>}</label>
      <label>Password <input name="password" type="password" required minLength={8} />
        {state.errors?.password && <small className="problem">{state.errors.password}</small>}</label>
      <button>Sign up</button>
    </form>
  );
}
