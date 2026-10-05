import { text } from '@sveltejs/kit';
const sid = (c) => {
  if (!c) return 'none';
  for (const p of c.split(';')) {
    const i = p.indexOf('=');
    if (i > 0 && p.slice(0, i).trim() === 'sid') return p.slice(i + 1).trim();
  }
  return 'none';
};
export function GET({ params, url, request }) {
  return text(`id=${params.id} q=${url.searchParams.get('q') ?? ''} sid=${sid(request.headers.get('cookie'))}`);
}
