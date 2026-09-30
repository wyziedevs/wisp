// Elysia on Bun; the pages from template literals.
import { Elysia } from 'elysia';
import { render } from '../fortunes.mjs';
import { renderPage } from '../page.mjs';

new Elysia()
    .get('/plaintext', () => 'Hello, World!')
    .get('/fortunes', ({ set }) => {
        set.headers['content-type'] = 'text/html; charset=utf-8';
        return render();
    })
    .get('/json', () => ({ message: 'Hello, World!' }))
    .get('/page', ({ set }) => {
        set.headers['content-type'] = 'text/html; charset=utf-8';
        return renderPage();
    })
    .listen({ hostname: '127.0.0.1', port: Number(process.env.PORT), reusePort: true });
