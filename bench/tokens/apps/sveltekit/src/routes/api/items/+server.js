// @feature api
import { json } from '@sveltejs/kit';
import { items } from '#lib/db';

export async function GET() {
	return json(await items());
}
