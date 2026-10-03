# Serve extras

All on the built-in server; each costs nothing for what does not use it.

- **gzip**: embedded files (`wisp.js`, app css, client modules, `static/` in
  a release build) of a text type are compressed once, on the first request
  from a client that sends `accept-encoding: gzip`, and that copy is sent
  after (`vary: accept-encoding`). Pages and API answers are not: a proxy or
  CDN does those better.
- **Range**: files answer `range: bytes=a-b`, `a-` and `-n` with 206 and
  `content-range`, and a range past the end with 416. One range only;
  several, or a malformed one, get the whole file. A range is of the file
  itself, never of its gzip. `if-range` is honored with the file's etag.
- **Security headers**: HTML pages and error pages carry
  `x-content-type-options: nosniff` and
  `referrer-policy: strict-origin-when-cross-origin` unless the app set its
  own. `WISP_HSTS=on` adds `strict-transport-security` (one year, subdomains)
  to every answer: for a site served over https by the proxy in front.
  `WISP_SECURE_HEADERS=off` leaves the first two out.
- **Health**: `GET /_wisp/health` is 200 `ok`, and 503 `stopping` once the
  server drains, so a load balancer moves traffic away first.
- **Handler timeout**: `WISP_HANDLER_TIMEOUT=30` (seconds) drops a handler
  that is still waiting then, and answers 503. It is checked when the
  handler waits; one that blocks its thread without awaiting cannot be
  stopped by anything.
- **OpenTelemetry**: `OTEL_EXPORTER_OTLP_ENDPOINT=http://localhost:4318`
  (or `OTEL_EXPORTER_OTLP_TRACES_ENDPOINT` for the whole address) sends one
  server span per request, as OTLP/HTTP JSON in batches from its own thread.
  `OTEL_SERVICE_NAME` names the service (default `wisp`); an incoming
  `traceparent` is continued. Plain http only (a collector beside the app);
  spans the collector cannot take are dropped, never waited for.
