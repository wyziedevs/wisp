// @feature form
import { fail, redirect } from '@sveltejs/kit';

const EMAIL = /^[^\s@]+@[^\s@]+\.[^\s@]+$/;

export const actions = {
	default: async ({ request }) => {
		const form = await request.formData();
		const name = String(form.get('name') ?? '');
		const email = String(form.get('email') ?? '');
		const errors = {};
		if (name.length < 1 || name.length > 50) errors.name = 'Name must be 1 to 50 characters';
		if (!EMAIL.test(email)) errors.email = 'Enter a valid email';
		if (errors.name || errors.email) return fail(422, { name, email, errors });
		console.log(`${name} <${email}>`);
		redirect(303, '/');
	}
};
