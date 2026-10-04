import { serve } from "@hono/node-server";
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

app.all("/*", (c) => {
  c.header("Server", "Hono");
  return c.text("Not Found", 404);
});

const port = parseInt(process.env.PORT || "8080");
const hostname = process.env.HOST || "0.0.0.0";
serve({ fetch: app.fetch, hostname, port }, (info) => {
  if (!info) {
    console.error(`Couldn't bind to http://${hostname}:${port}!`);
    process.exit(1);
  }
  console.log(`Successfully bound to http://${hostname}:${port}.`);
});
