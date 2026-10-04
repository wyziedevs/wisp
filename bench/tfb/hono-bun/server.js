// NOT TechEmpower source: TFB has no Bun entry for Hono. The same two routes as
// ../hono-node/src/server.js, served by Bun's own server (Hono's documented Bun entry:
// an object with `fetch`), one process per core sharing the port with reusePort.
import { Hono } from "hono";

const app = new Hono();
app
  .get("/plaintext", (c) => {
    c.header("Server", "Hono");
    return c.text("Hello, World!");
  })
  .get("/json", (c) => {
    c.header("Server", "Hono");
    return c.json({ message: "Hello, World!" });
  });

Bun.serve({
  port: parseInt(process.env.PORT || "8080"),
  hostname: "0.0.0.0",
  reusePort: true,
  fetch: app.fetch,
});
