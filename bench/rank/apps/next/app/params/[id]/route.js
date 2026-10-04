export const dynamic = 'force-dynamic';
export async function GET(req, { params }) {
  const { id } = await params;
  return new Response(`id=${id} q=${req.nextUrl.searchParams.get('q')} sid=${req.cookies.get('sid')?.value ?? 'none'}`, { headers: { 'content-type': 'text/plain; charset=utf-8' } });
}
