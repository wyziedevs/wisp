// @feature upload
import { users } from '@/lib/db';

export async function GET(request, { params }) {
  const { id } = await params;
  const avatar = users.find((u) => u.id == id)?.avatar;
  if (!avatar) return new Response('Not found', { status: 404 });
  return new Response(avatar.bytes, { headers: { 'content-type': avatar.type } });
}
