export const prerender = false;
export const GET = () => new Response('hello', { headers: { 'content-type': 'text/plain; charset=utf-8' } });
