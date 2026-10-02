import { json } from '@sveltejs/kit';

export async function POST({ request }) {
    const b = await request.json().catch(() => null);
    if (b === null || typeof b !== 'object') return json({ errors: ['body'] }, { status: 422 });
    const errors = [];
    if (typeof b.name !== 'string' || b.name.length < 1 || b.name.length > 50) errors.push('name');
    if (typeof b.email !== 'string' || !b.email.includes('@')) errors.push('email');
    if (!Number.isInteger(b.age) || b.age < 0 || b.age > 150) errors.push('age');
    if (!Array.isArray(b.tags) || b.tags.length > 10 || !b.tags.every((t) => typeof t === 'string')) errors.push('tags');
    if (errors.length) return json({ errors }, { status: 422 });
    return json({ name: b.name, email: b.email, age: b.age, tags: b.tags });
}
