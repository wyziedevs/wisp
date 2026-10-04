import { rows } from '../data.mjs';
export const prerender = false;
export const GET = () => Response.json(rows());
