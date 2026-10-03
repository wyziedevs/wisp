// Bun.serve with its route table (handlers, not static responses, so
// every request is answered afresh); the pages from template literals.
import { render } from '../fortunes.mjs';
import { renderPage } from '../page.mjs';

const json = (body, status = 200) => Response.json(body, { status });

const text = { 'Content-Type': 'text/plain; charset=utf-8' };
const html = { 'Content-Type': 'text/html; charset=utf-8' };

Bun.serve({
    hostname: '127.0.0.1',
    port: Number(process.env.PORT),
    reusePort: true,
    maxRequestBodySize: 8 * 1024 * 1024,
    routes: {
        '/plaintext': () => new Response('Hello, World!', { headers: text }),
        '/fortunes': () => new Response(render(), { headers: html }),
        '/json': () => Response.json({ message: 'Hello, World!' }),
        '/page': () => new Response(renderPage(), { headers: html }),
        // the-benchmarker's routes, as its Bun entry answers them.
        '/': () => new Response(null, { status: 204 }),
        '/user': () => new Response(null, { status: 204 }),
        '/user/:id': ({ params }) => new Response(params.id, { status: 200 }),
        // The practice routes (README): a wait, a validated body, an upload,
        // a list, a static file and a WebSocket.
        '/wait': async () => {
            await Bun.sleep(20);
            return json({ ok: true });
        },
        '/echo': {
            POST: async (req) => {
                const b = await req.json().catch(() => null);
                if (b === null || typeof b !== 'object') return json({ errors: ['body'] }, 422);
                const errors = [];
                if (typeof b.name !== 'string' || b.name.length < 1 || b.name.length > 50) errors.push('name');
                if (typeof b.email !== 'string' || !b.email.includes('@')) errors.push('email');
                if (!Number.isInteger(b.age) || b.age < 0 || b.age > 150) errors.push('age');
                if (!Array.isArray(b.tags) || b.tags.length > 10 || !b.tags.every((t) => typeof t === 'string')) errors.push('tags');
                if (errors.length) return json({ errors }, 422);
                return json({ name: b.name, email: b.email, age: b.age, tags: b.tags });
            },
        },
        '/upload': {
            POST: async (req) => new Response(String((await req.arrayBuffer()).byteLength), { headers: text }),
        },
        '/list': () => {
            const list = [];
            for (let i = 0; i < 1000; i++) list.push({ id: i, name: `user ${i}`, email: `user${i}@example.com`, active: i % 3 !== 0 });
            return json(list);
        },
        '/static/app.js': Bun.file(new URL('../../static/app.js', import.meta.url)),
        '/ws': (req, server) => (server.upgrade(req) ? undefined : new Response('Upgrade Required', { status: 426 })),
    },
    websocket: {
        message: (ws, message) => ws.send(message),
    },
    fetch: () => new Response('Not Found', { status: 404 }),
});
