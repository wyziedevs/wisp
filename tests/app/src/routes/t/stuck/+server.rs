// `TIMEOUT`: a handler that never answers is a 503 after a second.

const TIMEOUT: u32 = 1;

async fn get() -> String {
    std::future::pending::<()>().await;
    String::new()
}
