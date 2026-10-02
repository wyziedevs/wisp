import adapter from '@sveltejs/adapter-node';

// trustedOrigins: /upload takes a body of any type, and Kit refuses a
// text/plain or form POST that carries no matching Origin header.
export default { kit: { adapter: adapter(), csrf: { trustedOrigins: ['*'] } } };
