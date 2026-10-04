// @feature form
import { fail, redirect } from '@sveltejs/kit';

export const actions = {
	default: async ({ request }) => {
		const form = Object.fromEntries(await request.formData());
		const errors = {};
		if (!form.name || form.name.length > 50) errors.name = 'must have 1 to 50 characters';
		if (!/^\S+@\S+\.\S+$/.test(form.email)) errors.email = 'must be an email address';
		if (Object.keys(errors).length) return fail(422, { ...form, errors });
		console.log(`${form.name} <${form.email}>`);
		redirect(303, '/');
	}
};
