import { setTimeout as sleep } from 'node:timers/promises';

export const dynamic = 'force-dynamic';

export async function GET() {
    await sleep(20);
    return Response.json({ ok: true });
}
