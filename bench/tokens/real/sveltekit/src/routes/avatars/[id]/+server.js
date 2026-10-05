// @feature upload
import { error } from '@sveltejs/kit';
import { users } from '#lib/server/db';

export function GET({ params }) {
	const avatar = users.find((u) => u.id == params.id)?.avatar;
	if (!avatar) error(404);
	return new Response(avatar.bytes, { headers: { 'content-type': avatar.type } });
}
