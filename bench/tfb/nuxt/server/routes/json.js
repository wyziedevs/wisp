export default defineEventHandler((event) => {
  setResponseHeaders(event, { 'content-type': 'application/json', server: 'Nuxt' });
  return { message: 'Hello, World!' };
});
