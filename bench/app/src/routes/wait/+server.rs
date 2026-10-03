// A database call's stand-in: a 20 ms wait that does not hold the thread.

#[derive(Json)]
struct Done {
    ok: bool,
}

async fn get() -> Done {
    wisp::sleep(std::time::Duration::from_millis(20)).await;
    Done { ok: true }
}
