// Fastify, the pages from template literals (fortunes.mjs, page.mjs).
import Fastify from 'fastify';
import fastifyStatic from '@fastify/static';
import fastifyWebsocket from '@fastify/websocket';
import { setTimeout as sleep } from 'node:timers/promises';
import { fileURLToPath } from 'node:url';
import { render } from '../fortunes.mjs';
import { renderPage } from '../page.mjs';

const app = Fastify({ bodyLimit: 8 * 1024 * 1024, ajv: { customOptions: { allErrors: true } } });
app.get('/plaintext', async () => 'Hello, World!');
app.get('/fortunes', async (req, reply) => {
    reply.type('text/html; charset=utf-8');
    return render();
});
app.get('/json', async () => ({ message: 'Hello, World!' }));
app.get('/page', async (req, reply) => {
    reply.type('text/html; charset=utf-8');
    return renderPage();
});
// the-benchmarker's routes, as its Fastify entry answers them.
app.get('/', (req, reply) => {
    reply.send();
});
app.get('/user/:id', (req, reply) => {
    reply.send(req.params.id);
});
app.post('/user', (req, reply) => {
    reply.send();
});

// The practice routes (README): a wait, a validated body, an upload, a list,
// a static file and a WebSocket.
app.get('/wait', async () => {
    await sleep(20);
    return { ok: true };
});
const fields = ['name', 'email', 'age', 'tags'];
app.post('/echo', {
    schema: {
        body: {
            type: 'object',
            required: fields,
            properties: {
                name: { type: 'string', minLength: 1, maxLength: 50 },
                email: { type: 'string', pattern: '@' },
                age: { type: 'integer', minimum: 0, maximum: 150 },
                tags: { type: 'array', items: { type: 'string' }, maxItems: 10 },
            },
        },
    },
    // Validation and body-parse failures as {"errors":[field, ...]}.
    errorHandler: (err, req, reply) => {
        if (err.validation) {
            const bad = new Set(err.validation.map((e) => e.instancePath.split('/')[1] ?? e.params.missingProperty));
            return reply.code(422).send({ errors: fields.filter((f) => bad.has(f)) });
        }
        if (err.statusCode === 400) return reply.code(422).send({ errors: ['body'] });
        reply.send(err);
    },
}, async (req) => {
    const { name, email, age, tags } = req.body;
    return { name, email, age, tags };
});
// Its own context, so any content type is a buffer here, JSON included.
app.register(async (upload) => {
    upload.removeAllContentTypeParsers();
    upload.addContentTypeParser('*', { parseAs: 'buffer' }, (req, body, done) => done(null, body));
    upload.post('/upload', async (req, reply) => {
        reply.type('text/plain');
        return String(req.body?.length ?? 0);
    });
});
app.get('/list', async () => {
    const list = [];
    for (let i = 0; i < 1000; i++) list.push({ id: i, name: `user ${i}`, email: `user${i}@example.com`, active: i % 3 !== 0 });
    return list;
});
await app.register(fastifyStatic, { root: fileURLToPath(new URL('../../static', import.meta.url)), prefix: '/static/' });
await app.register(fastifyWebsocket);
app.get('/ws', { websocket: true }, (socket) => {
    socket.on('message', (data, isBinary) => socket.send(data, { binary: isBinary }));
});
await app.listen({ host: '127.0.0.1', port: Number(process.env.PORT) });
