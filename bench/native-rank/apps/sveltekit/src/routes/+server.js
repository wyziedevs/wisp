import { text } from '@sveltejs/kit';
export function GET() {
  return text('Hello, World!', { headers: { 'content-type': 'text/plain; charset=utf-8' } });
}
