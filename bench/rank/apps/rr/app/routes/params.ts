import type { LoaderFunctionArgs } from 'react-router';
const sid = (h: string | null) => h?.match(/(?:^|; )sid=([^;]*)/)?.[1];
export const loader = ({ request, params }: LoaderFunctionArgs) =>
  new Response(`id=${params.id} q=${new URL(request.url).searchParams.get('q')} sid=${sid(request.headers.get('cookie')) ?? 'none'}`, { headers: { 'content-type': 'text/plain; charset=utf-8' } });
