// @feature auth
import { redirect } from '@sveltejs/kit';
import { sessions } from '$lib/server/db';

export function load({ locals }) {
	if (!locals.user) redirect(303, '/login');
	const { id, email, avatar } = locals.user;
	return { id, email, avatar: !!avatar };
}

export const actions = {
	logout: ({ cookies }) => {
		sessions.delete(cookies.get('session'));
		cookies.delete('session', { path: '/' });
		redirect(303, '/login');
	}
};
