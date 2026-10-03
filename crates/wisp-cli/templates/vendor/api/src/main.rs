// A JSON API: every route is a `+server.rs` under src/routes/api. The data
// lives in src/routes/api/notes/+server.rs; src/hooks.rs checks the API key and allows other
// sites to call it. `wisp dev`, then open http://127.0.0.1:3000/_wisp/docs.
wisp::main!();

#[cfg(test)]
mod tests;
