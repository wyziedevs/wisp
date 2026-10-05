// Hono on Bun (Hono's documented Bun entry: an object with `fetch`). Bun is single-threaded per
// process; run.sh starts one process per core sharing the port with reusePort (as the TFB entries do).
import { Hono } from 'hono';

const esc = (s) => s.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;');
const sid = (c) => {
  if (!c) return 'none';
  for (const p of c.split(';')) {
    const i = p.indexOf('=');
    if (i > 0 && p.slice(0, i).trim() === 'sid') return p.slice(i + 1).trim();
  }
  return 'none';
};

const app = new Hono();
app
  .get('/', (c) => c.text('Hello, World!'))
  .get('/json', (c) => c.json({ message: 'Hello, World!' }))
  .get('/params/:id', (c) => c.text(`id=${c.req.param('id')} q=${c.req.query('q') ?? ''} sid=${sid(c.req.header('cookie'))}`))
  .get('/list', (c) => {
    let s = '<!DOCTYPE html><html><head></head><body><h1>List</h1><ul>';
    for (let i = 1; i <= 1000; i++) s += `<li>${esc(`Item <${i}> & co`)}</li>`;
    return c.html(s + '</ul></body></html>');
  })
  .get('/json-big', (c) => {
    const a = [];
    for (let i = 1; i <= 200; i++) a.push({ id: i, name: `user-${i}`, active: i % 3 !== 0, score: (i * 37) % 101, tags: ['a', `t${i % 7}`] });
    return c.json(a);
  });

Bun.serve({ port: parseInt(process.env.PORT || '8080'), hostname: '0.0.0.0', reusePort: true, fetch: app.fetch });
