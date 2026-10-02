// The 8 MiB limit is adapter-node's BODY_SIZE_LIMIT=8M, set when the server starts.
export async function POST({ request }) {
    const body = await request.arrayBuffer();
    return new Response(String(body.byteLength), { headers: { 'content-type': 'text/plain' } });
}
