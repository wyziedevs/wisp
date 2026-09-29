use std::sync::Mutex;
use std::time::Duration;

static ITEMS: Mutex<Vec<String>> = Mutex::new(Vec::new());

struct Data {
    items: Vec<String>,
}

fn load() -> Data {
    Data { items: ITEMS.lock().unwrap().clone() }
}

#[action]
async fn add(text: String) {
    wisp::sleep(Duration::from_millis(300)).await;
    ITEMS.lock().unwrap().push(text);
}

#[action]
fn answer() -> Response {
    Response::json("{\"n\":42}")
}
