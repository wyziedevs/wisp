export default defineEventHandler((event) => {
  setResponseHeaders(event, { 'content-type': 'text/plain', server: 'Nuxt' });
  return 'Hello, World!';
});
