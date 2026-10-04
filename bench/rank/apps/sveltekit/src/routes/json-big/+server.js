import { json } from '@sveltejs/kit';
import { rows } from '../../lib/data.mjs';
export const GET = () => json(rows());
