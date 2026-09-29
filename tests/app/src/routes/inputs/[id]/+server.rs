use std::sync::Mutex;

/// Names posted here, in order.
static NAMES: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// What a request said, read by name: the route's `id`, then the query.
#[derive(Json)]
struct Echo {
    id: u32,
    q: Option<String>,
    n: Vec<u8>,
    on: bool,
    names: Vec<String>,
}

/// A value, so it is sent as JSON.
fn get(id: u32, q: Option<String>, n: Vec<u8>, on: bool) -> Echo {
    Echo { id, q, n, on, names: NAMES.lock().unwrap().clone() }
}

/// A form's `name`. Nothing to answer, so a 204.
fn post(name: String) {
    NAMES.lock().unwrap().push(name);
}
