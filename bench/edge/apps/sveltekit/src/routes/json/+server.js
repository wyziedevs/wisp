import { json } from '@sveltejs/kit';
export const GET = () => json({ ok: true, name: 'sveltekit', n: 42 });
