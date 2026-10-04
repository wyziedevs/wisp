fn init() {
    // A job that fails is tried again later; the queue is a saved table.
    wisp::work("greet", |name: String| async move {
        println!("hello, {name}");
        Ok(())
    });
    // UTC. A string literal, so `wisp build` can write it into the host's cron.
    wisp::cron("0 3 * * *", || async {
        println!("03:00: time to clean up");
    });
}
