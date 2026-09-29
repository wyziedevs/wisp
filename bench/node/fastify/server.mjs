// Fastify rendering with a template literal: no template engine, the
// fastest way to produce HTML in Node.
import Fastify from 'fastify';
import { load } from '../fortunes.mjs';

const escapes = { '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' };
const escape = (s) => s.replace(/[&<>"']/g, (c) => escapes[c]);

const app = Fastify();
app.get('/plaintext', async () => 'Hello, World!');
app.get('/fortunes', async (req, reply) => {
    const rows = load().map((f) => `<tr><td>${f.id}</td><td>${escape(f.message)}</td></tr>`).join('\n');
    reply.type('text/html; charset=utf-8');
    return `<!DOCTYPE html>
<html>
<head><title>Fortunes</title></head>
<body><table>
<tr><th>id</th><th>message</th></tr>
${rows}
</table></body>
</html>
`;
});
await app.listen({ host: '127.0.0.1', port: Number(process.env.PORT) });
