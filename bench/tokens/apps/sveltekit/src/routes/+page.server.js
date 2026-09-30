// @feature list
import { items } from '$lib/db';

export async function load() {
	return { items: await items() };
}
