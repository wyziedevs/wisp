import { app } from './hono-app.mjs';
Bun.serve({ port: Number(process.env.PORT), fetch: app.fetch });
