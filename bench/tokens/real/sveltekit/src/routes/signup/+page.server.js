// @feature auth
import { fail, redirect } from '@sveltejs/kit';
import { hash } from '@node-rs/argon2';
import { users, startSession } from '$lib/server/db';

export const actions = {
	default: async ({ request, cookies }) => {
		const { email, password } = Object.fromEntries(await request.formData());
		const errors = {};
		if (!/^\S+@\S+\.\S+$/.test(email)) errors.email = 'Enter a valid email';
		else if (users.some((u) => u.email === email)) errors.email = 'Already signed up';
		if (password.length < 8) errors.password = 'At least 8 characters';
		if (errors.email || errors.password) return fail(422, { email, errors });
		const user = { id: users.length + 1, email, hash: await hash(password) };
		users.push(user);
		startSession(cookies, user);
		redirect(303, '/dashboard');
	}
};
