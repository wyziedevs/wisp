import { Router, json } from 'itty-router';
import { items, page, rows, cookie } from './data.mjs';
const router = Router();
const html = (s) => new Response(s, { headers: { 'content-type': 'text/html; charset=utf-8' } });
router
  .get('/', () => new Response('hello'))
  .get('/list1000', () => html(page(items(1000))))
  .get('/json-big', () => json(rows()))
  .get('/params/:id', (req) => new Response(`id=${req.params.id} q=${req.query.q} sid=${cookie(req.headers.get('cookie'), 'sid') ?? 'none'}`));
export default { fetch: router.fetch };
