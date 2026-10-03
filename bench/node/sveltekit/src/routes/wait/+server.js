import { json } from '@sveltejs/kit';
import { setTimeout as sleep } from 'node:timers/promises';

export async function GET() {
    await sleep(20);
    return json({ ok: true });
}
