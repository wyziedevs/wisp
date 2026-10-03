import { Hono } from 'hono';
const esc = (s) => s.replace(/[&<>"']/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' })[c]);
const items = Array.from({ length: 50 }, (_, i) => `Item <${i + 1}> & co`);
export const app = new Hono();
app.get('/', (c) => c.text('hello'));
app.get('/list', (c) => c.html(`<!doctype html><html lang="en"><head><meta charset="utf-8"><title>List</title></head><body><h1>List</h1><ul>${items.map((s) => `<li>${esc(s)}</li>`).join('')}</ul></body></html>`));
app.get('/json', (c) => c.json({ ok: true, name: 'hono', n: 42 }));
export default app;
