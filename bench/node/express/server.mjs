// Express with its defaults (ETag and X-Powered-By on), as apps run it;
// the pages from template literals (fortunes.mjs, page.mjs).
import express from 'express';
import { WebSocketServer } from 'ws';
import { setTimeout as sleep } from 'node:timers/promises';
import { fileURLToPath } from 'node:url';
import { render } from '../fortunes.mjs';
import { renderPage } from '../page.mjs';

const app = express();
const MiB = 1024 * 1024;
app.get('/plaintext', (req, res) => res.type('text/plain').send('Hello, World!'));
app.get('/fortunes', (req, res) => res.type('text/html').send(render()));
app.get('/page', (req, res) => res.type('text/html').send(renderPage()));
app.get('/json', (req, res) => res.json({ message: 'Hello, World!' }));
// the-benchmarker's routes, as its Express entry answers them.
app.get('/', (req, res) => res.send(''));
app.get('/user/:id', (req, res) => res.send(req.params.id));
app.post('/user', (req, res) => res.send(''));

// The practice routes (README): a wait, a validated body, an upload, a list,
// a static file and a WebSocket.
app.get('/wait', async (req, res) => {
    await sleep(20);
    res.json({ ok: true });
});
app.post('/echo', express.json(), (req, res) => {
    const b = req.body ?? {};
    const errors = [];
    if (typeof b.name !== 'string' || b.name.length < 1 || b.name.length > 50) errors.push('name');
    if (typeof b.email !== 'string' || !b.email.includes('@')) errors.push('email');
    if (!Number.isInteger(b.age) || b.age < 0 || b.age > 150) errors.push('age');
    if (!Array.isArray(b.tags) || b.tags.length > 10 || !b.tags.every((t) => typeof t === 'string')) errors.push('tags');
    if (errors.length) return res.status(422).json({ errors });
    res.json({ name: b.name, email: b.email, age: b.age, tags: b.tags });
});
app.post('/upload', express.raw({ type: () => true, limit: 8 * MiB }), (req, res) => {
    res.type('text/plain').send(String(req.body?.length ?? 0));
});
app.get('/list', (req, res) => {
    const list = [];
    for (let i = 0; i < 1000; i++) list.push({ id: i, name: `user ${i}`, email: `user${i}@example.com`, active: i % 3 !== 0 });
    res.json(list);
});
app.use('/static', express.static(fileURLToPath(new URL('../../static', import.meta.url))));
// A body that does not parse is a 422 too, not body-parser's 400.
app.use((err, req, res, next) => {
    if (req.path === '/echo' && err.status === 400) return res.status(422).json({ errors: ['body'] });
    next(err);
});

const server = app.listen(Number(process.env.PORT), '127.0.0.1');
new WebSocketServer({ server, path: '/ws' }).on('connection', (ws) => {
    ws.on('message', (data, isBinary) => ws.send(data, { binary: isBinary }));
});
