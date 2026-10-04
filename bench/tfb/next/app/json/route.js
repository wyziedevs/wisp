export const dynamic = 'force-dynamic';
export function GET() {
  return Response.json({ message: 'Hello, World!' }, { headers: { server: 'Next.js' } });
}
