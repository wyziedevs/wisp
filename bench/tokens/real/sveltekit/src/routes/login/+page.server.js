// @feature auth
import { fail, redirect } from '@sveltejs/kit';
import { verify } from '@node-rs/argon2';
import { users, startSession } from '$lib/server/db';

export const actions = {
	default: async ({ request, cookies }) => {
		const { email, password } = Object.fromEntries(await request.formData());
		const user = users.find((u) => u.email === email);
		if (!user || !(await verify(user.hash, password))) {
			return fail(422, { email, error: 'Wrong email or password' });
		}
		startSession(cookies, user);
		redirect(303, '/dashboard');
	}
};
