export const dynamic = 'force-dynamic';
const sid = (c) => {
  if (!c) return 'none';
  for (const p of c.split(';')) {
    const i = p.indexOf('=');
    if (i > 0 && p.slice(0, i).trim() === 'sid') return p.slice(i + 1).trim();
  }
  return 'none';
};
export async function GET(request, { params }) {
  const { id } = await params;
  const q = new URL(request.url).searchParams.get('q') ?? '';
  return new Response(`id=${id} q=${q} sid=${sid(request.headers.get('cookie'))}`, { headers: { 'content-type': 'text/plain; charset=utf-8' } });
}
