// NOT TechEmpower source: TFB has no Bun entry for Hono. The same two routes as
// ../hono-node/src/server.js, served by Bun's own server (Hono's documented Bun entry:
// an object with `fetch`), one process per core sharing the port with reusePort.
import { Hono } from "hono";

const app = new Hono();
app
  .get("/plaintext", (c) => {
    c.header("Server", "Hono");
    return c.body("Hello, World!", 200, { "Content-Type": "text/plain" });
  })
  .get("/json", (c) => {
    c.header("Server", "Hono");
    return c.body(JSON.stringify({ message: "Hello, World!" }), 200, { "Content-Type": "application/json" });
  });

Bun.serve({
  port: parseInt(process.env.PORT || "8080"),
  hostname: "0.0.0.0",
  reusePort: true,
  fetch: app.fetch,
});
