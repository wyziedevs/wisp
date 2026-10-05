export const dynamic = 'force-dynamic';
export function GET() {
  const a = [];
  for (let i = 1; i <= 200; i++) a.push({ id: i, name: `user-${i}`, active: i % 3 !== 0, score: (i * 37) % 101, tags: ['a', `t${i % 7}`] });
  return Response.json(a);
}
