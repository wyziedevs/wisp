/// How many greetings wait.
fn get() -> String {
    format!("{} waiting\n", wisp::queue("greet").pending())
}

/// Queues a greeting; a worker says it, as soon as it is free.
fn post() -> &'static str {
    wisp::queue("greet").push(&"world".to_string());
    "queued\n"
}
