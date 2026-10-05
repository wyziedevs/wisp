// @feature auth
import { sessions } from '#lib/server/db';

export async function handle({ event, resolve }) {
	event.locals.user = sessions.get(event.cookies.get('session'));
	return resolve(event);
}
