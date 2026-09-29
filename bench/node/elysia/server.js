// Elysia on Bun; the page from a template literal.
import { Elysia } from 'elysia';
import { render } from '../fortunes.mjs';

new Elysia()
    .get('/plaintext', () => 'Hello, World!')
    .get('/fortunes', ({ set }) => {
        set.headers['content-type'] = 'text/html; charset=utf-8';
        return render();
    })
    .get('/json', () => ({ message: 'Hello, World!' }))
    .listen({ hostname: '127.0.0.1', port: Number(process.env.PORT), reusePort: true });
