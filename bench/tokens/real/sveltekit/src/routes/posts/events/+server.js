// @feature live
import { listeners } from '#lib/server/db';

export function GET() {
	let send;
	const stream = new ReadableStream({
		start(controller) {
			send = () => controller.enqueue('data: new\n\n');
			listeners.add(send);
		},
		cancel() {
			listeners.delete(send);
		}
	});
	return new Response(stream, { headers: { 'content-type': 'text/event-stream' } });
}
