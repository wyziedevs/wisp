import { rows } from '../data.mjs';
export const loader = () => Response.json(rows());
