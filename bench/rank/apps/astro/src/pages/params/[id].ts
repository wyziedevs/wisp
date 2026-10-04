import type { APIRoute } from 'astro';
export const prerender = false;
export const GET: APIRoute = ({ params, url, cookies }) =>
  new Response(`id=${params.id} q=${url.searchParams.get('q')} sid=${cookies.get('sid')?.value ?? 'none'}`, { headers: { 'content-type': 'text/plain; charset=utf-8' } });
