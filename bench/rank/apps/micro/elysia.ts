import { Elysia } from 'elysia';
import { items, page, rows } from './data.mjs';
new Elysia()
  .get('/', () => 'hello')
  .get('/list1000', ({ set }) => { set.headers['content-type'] = 'text/html; charset=utf-8'; return page(items(1000)); })
  .get('/json-big', () => rows())
  .get('/params/:id', ({ params, query, cookie }) => `id=${params.id} q=${query.q} sid=${cookie.sid.value ?? 'none'}`)
  .listen({ port: Number(process.env.PORT), hostname: '127.0.0.1' });
