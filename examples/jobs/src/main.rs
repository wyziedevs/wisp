// A queue and a cron schedule: src/hooks.rs registers both, and every host
// runs them. The binary and Docker run them in the process; `wisp build
// --target cloudflare` (or vercel, netlify) writes the schedule into the
// host's cron triggers, which run it and the queue's due jobs. There set
// CRON_SECRET and WISP_STORE (the queue is a table). `wisp dev`, then POST /.
wisp::main!();
