const fastify = require("fastify")({ logger: false, keepAliveTimeout: 0 });

fastify.setErrorHandler((error, request, reply) => {
  console.log(error)
  reply.status(500).send({ ok: false })
})

fastify.addHook('onRequest', (request, reply, done) => {
  reply.header("Server", "Fastify");
  done()
})

fastify.get("/json", {
  schema: {
    response: {
      200: {
        type: 'object',
        properties: {
          message: { type: 'string' }
        }
      }
    }
  }
}, (req, reply) => {
  reply
    .header("Content-Type", "application/json")
    .code(200)
    .send({ message: "Hello, World!" });
});

fastify.get("/plaintext", (req, reply) => {
  reply
    .header("Content-Type", "text/plain")
    .code(200)
    .send("Hello, World!");
});

fastify.listen({ port: 8080, host: "0.0.0.0" }, (err, address) => {
  if (err) {
    throw err;
  }

  console.log(
    `Worker started and listening on ${address} ${new Date().toISOString()}`
  );
});
