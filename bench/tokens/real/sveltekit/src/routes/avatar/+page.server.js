// @feature upload
import { error, fail, redirect } from '@sveltejs/kit';

export function load({ locals }) {
	if (!locals.user) redirect(303, '/login');
}

export const actions = {
	default: async ({ request, locals }) => {
		if (!locals.user) error(401);
		const file = (await request.formData()).get('avatar');
		if (!file?.type?.startsWith('image/')) return fail(422, { error: 'Choose an image' });
		if (file.size > 1024 * 1024) return fail(422, { error: 'At most 1 MB' });
		locals.user.avatar = { type: file.type, bytes: new Uint8Array(await file.arrayBuffer()) };
		redirect(303, '/dashboard');
	}
};
