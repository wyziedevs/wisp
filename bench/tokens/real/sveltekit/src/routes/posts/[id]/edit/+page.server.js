// @feature crud
import { error, fail, redirect } from '@sveltejs/kit';
import { posts, checkPost } from '#lib/server/db';

function find(params) {
	const post = posts.get(Number(params.id));
	if (!post) error(404, 'Not found');
	return post;
}

export function load({ params }) {
	return { post: find(params) };
}

export const actions = {
	default: async ({ request, params }) => {
		const post = find(params);
		const { title, body } = Object.fromEntries(await request.formData());
		const errors = checkPost({ title, body });
		if (errors) return fail(422, { title, body, errors });
		Object.assign(post, { title, body });
		redirect(303, '/posts');
	}
};
