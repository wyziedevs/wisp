// uWebSockets.js: Node with uWS's C++ HTTP server in place of node:http.
// Each cluster.mjs worker binds the port itself (SO_REUSEPORT on Linux),
// so the kernel spreads connections. No microcaching, which would answer
// from a cache the other servers do not have.
import { createRequire } from 'node:module';
import { render } from '../fortunes.mjs';
import { renderPage } from '../page.mjs';

// Its ESM wrapper imports a file the package does not ship; require works.
const uWS = createRequire(import.meta.url)('uWebSockets.js');
// No "uWebSockets: 20" header, as its the-benchmarker entry runs.
uWS._cfg('silent');

uWS.App()
    .get('/plaintext', (res) => res.writeHeader('Content-Type', 'text/plain; charset=utf-8').end('Hello, World!'))
    .get('/fortunes', (res) => res.writeHeader('Content-Type', 'text/html; charset=utf-8').end(render()))
    .get('/json', (res) => res.writeHeader('Content-Type', 'application/json').end(JSON.stringify({ message: 'Hello, World!' })))
    .get('/page', (res) => res.writeHeader('Content-Type', 'text/html; charset=utf-8').end(renderPage()))
    // the-benchmarker's routes as its entry answers them: declarative
    // responses, written in C++ without entering JavaScript.
    .get('/', new uWS.DeclarativeResponse().end())
    .get('/user/:id', new uWS.DeclarativeResponse().writeParameterValue('id').end())
    .post('/user', new uWS.DeclarativeResponse().end())
    .listen('127.0.0.1', Number(process.env.PORT), (socket) => {
        if (!socket) {
            console.error('uws: cannot listen');
            process.exit(1);
        }
    });
