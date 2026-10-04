import { app } from './hono-app.mjs';
Deno.serve({ port: Number(Deno.env.get('PORT')) }, app.fetch);
