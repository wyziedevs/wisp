// A route handler has no body limit setting (Next.js's bodySizeLimit is for
// Server Actions), so the 8 MiB is checked here.
const LIMIT = 8 * 1024 * 1024;

export async function POST(request) {
    if (Number(request.headers.get('content-length')) > LIMIT) return new Response('Payload Too Large', { status: 413 });
    const body = await request.arrayBuffer();
    if (body.byteLength > LIMIT) return new Response('Payload Too Large', { status: 413 });
    return new Response(String(body.byteLength), { headers: { 'content-type': 'text/plain' } });
}
