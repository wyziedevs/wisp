const express = require('express');
const app = express();
app.disable('x-powered-by');
app.set('etag', false); // production tuning as TechEmpower's entry; the ETag hash is per-request work

const esc = (s) => s.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;');
const sid = (c) => {
  if (!c) return 'none';
  for (const p of c.split(';')) {
    const i = p.indexOf('=');
    if (i > 0 && p.slice(0, i).trim() === 'sid') return p.slice(i + 1).trim();
  }
  return 'none';
};

app.get('/', (req, res) => res.type('text/plain; charset=utf-8').send('Hello, World!'));
app.get('/json', (req, res) => res.json({ message: 'Hello, World!' }));
app.get('/params/:id', (req, res) =>
  res.type('text/plain; charset=utf-8').send(`id=${req.params.id} q=${req.query.q ?? ''} sid=${sid(req.headers.cookie)}`));
app.get('/list', (req, res) => {
  let s = '<!DOCTYPE html><html><head></head><body><h1>List</h1><ul>';
  for (let i = 1; i <= 1000; i++) s += `<li>${esc(`Item <${i}> & co`)}</li>`;
  res.type('text/html; charset=utf-8').send(s + '</ul></body></html>');
});
app.get('/json-big', (req, res) => {
  const a = [];
  for (let i = 1; i <= 200; i++) a.push({ id: i, name: `user-${i}`, active: i % 3 !== 0, score: (i * 37) % 101, tags: ['a', `t${i % 7}`] });
  res.json(a);
});

app.listen(+(process.env.PORT || 8080), '0.0.0.0');
