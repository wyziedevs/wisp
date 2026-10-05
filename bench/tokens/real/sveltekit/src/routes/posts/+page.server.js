// @feature crud
import { posts } from '#lib/server/db';

export function load({ url }) {
	const page = Math.max(1, Number(url.searchParams.get('page')) || 1);
	const all = [...posts.values()].reverse();
	return { page, posts: all.slice(page * 10 - 10, page * 10), more: all.length > page * 10 };
}

export const actions = {
	delete: async ({ request }) => {
		posts.delete(Number((await request.formData()).get('id')));
	}
};
