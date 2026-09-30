// @feature api
import { items } from '@/lib/db';

export async function GET() {
  return Response.json(await items());
}
