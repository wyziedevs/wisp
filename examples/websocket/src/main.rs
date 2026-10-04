// A WebSocket echo that numbers what each connection has sent. The same
// handler runs in the binary and on `wisp build --target` node, bun, deno,
// cloudflare and pages (wispweb.dev/docs/deploy); vercel, netlify and lambda cannot
// hold a socket and answer /ws with 501. On Cloudflare a connection lives in
// one isolate: state shared by every connection is a Durable Object's job.
wisp::main!();
