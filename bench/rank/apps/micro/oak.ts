import { Application, Router } from 'jsr:@oak/oak';
import { items, page, rows } from './data.mjs';
const router = new Router();
router
  .get('/', (ctx) => { ctx.response.body = 'hello'; })
  .get('/list1000', (ctx) => { ctx.response.type = 'html'; ctx.response.body = page(items(1000)); })
  .get('/json-big', (ctx) => { ctx.response.body = rows(); })
  .get('/params/:id', async (ctx) => { ctx.response.body = `id=${ctx.params.id} q=${ctx.request.url.searchParams.get('q')} sid=${(await ctx.cookies.get('sid')) ?? 'none'}`; });
const app = new Application();
app.use(router.routes());
await app.listen({ port: Number(Deno.env.get('PORT')), hostname: '127.0.0.1' });
