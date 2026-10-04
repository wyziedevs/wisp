import { App } from "fresh";
import { rows } from "./data.mjs";

export const app = new App();

app.get("/", () => new Response("hello"));
app.get("/json-big", () => Response.json(rows()));
app.get("/params/:id", (ctx) => {
  const sid = ctx.req.headers.get("cookie")?.match(/(?:^|; )sid=([^;]*)/)?.[1];
  return new Response(`id=${ctx.params.id} q=${ctx.url.searchParams.get("q")} sid=${sid ?? "none"}`);
});

// /list1000 is a file route (routes/list1000.tsx)
app.fsRoutes();
