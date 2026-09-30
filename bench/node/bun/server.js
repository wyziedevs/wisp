// Bun.serve with its route table (handlers, not static responses, so
// every request is answered afresh); the pages from template literals.
import { render } from '../fortunes.mjs';
import { renderPage } from '../page.mjs';

const text = { 'Content-Type': 'text/plain; charset=utf-8' };
const html = { 'Content-Type': 'text/html; charset=utf-8' };

Bun.serve({
    hostname: '127.0.0.1',
    port: Number(process.env.PORT),
    reusePort: true,
    routes: {
        '/plaintext': () => new Response('Hello, World!', { headers: text }),
        '/fortunes': () => new Response(render(), { headers: html }),
        '/json': () => Response.json({ message: 'Hello, World!' }),
        '/page': () => new Response(renderPage(), { headers: html }),
    },
    fetch: () => new Response('Not Found', { status: 404 }),
});
