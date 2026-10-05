// @feature crud
import { fail, redirect } from '@sveltejs/kit';
import { posts, nextId, checkPost, listeners } from '#lib/server/db';

export const actions = {
	default: async ({ request }) => {
		const { title, body } = Object.fromEntries(await request.formData());
		const errors = checkPost({ title, body });
		if (errors) return fail(422, { title, body, errors });
		const id = nextId();
		posts.set(id, { id, title, body });
		// @feature live
		for (const send of listeners) send();
		// @feature crud
		redirect(303, '/posts');
	}
};
