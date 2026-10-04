import { isbot } from 'isbot';
import { renderToReadableStream } from 'react-dom/server';
import { ServerRouter, type EntryContext } from 'react-router';

export default async function handleRequest(request: Request, status: number, headers: Headers, routerContext: EntryContext) {
  const body = await renderToReadableStream(<ServerRouter context={routerContext} url={request.url} />, {
    onError() { status = 500; },
  });
  if (isbot(request.headers.get('user-agent') || '')) await body.allReady;
  headers.set('Content-Type', 'text/html');
  return new Response(body, { headers, status });
}
