import { rows } from '../../lib/data.mjs';
export const dynamic = 'force-dynamic';
export const GET = () => Response.json(rows());
