const fastify = require('fastify')({ logger: false });

const esc = (s) => s.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;');
const sid = (c) => {
  if (!c) return 'none';
  for (const p of c.split(';')) {
    const i = p.indexOf('=');
    if (i > 0 && p.slice(0, i).trim() === 'sid') return p.slice(i + 1).trim();
  }
  return 'none';
};

fastify.get('/', (req, reply) => reply.type('text/plain; charset=utf-8').send('Hello, World!'));
fastify.get('/json', () => ({ message: 'Hello, World!' }));
fastify.get('/params/:id', (req, reply) =>
  reply.type('text/plain; charset=utf-8').send(`id=${req.params.id} q=${req.query.q ?? ''} sid=${sid(req.headers.cookie)}`));
fastify.get('/list', (req, reply) => {
  let s = '<!DOCTYPE html><html><head></head><body><h1>List</h1><ul>';
  for (let i = 1; i <= 1000; i++) s += `<li>${esc(`Item <${i}> & co`)}</li>`;
  return reply.type('text/html; charset=utf-8').send(s + '</ul></body></html>');
});
fastify.get('/json-big', () => {
  const a = [];
  for (let i = 1; i <= 200; i++) a.push({ id: i, name: `user-${i}`, active: i % 3 !== 0, score: (i * 37) % 101, tags: ['a', `t${i % 7}`] });
  return a;
});

fastify.listen({ port: +(process.env.PORT || 8080), host: '0.0.0.0' });
