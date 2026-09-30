// Fastify, the pages from template literals (fortunes.mjs, page.mjs).
import Fastify from 'fastify';
import { render } from '../fortunes.mjs';
import { renderPage } from '../page.mjs';

const app = Fastify();
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
await app.listen({ host: '127.0.0.1', port: Number(process.env.PORT) });
