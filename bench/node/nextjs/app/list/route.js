export const dynamic = 'force-dynamic';

export function GET() {
    const list = [];
    for (let i = 0; i < 1000; i++) list.push({ id: i, name: `user ${i}`, email: `user${i}@example.com`, active: i % 3 !== 0 });
    return Response.json(list);
}
