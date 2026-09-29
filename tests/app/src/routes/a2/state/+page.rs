use std::sync::atomic::{AtomicU32, Ordering};

static COUNT: AtomicU32 = AtomicU32::new(0);

struct Data {
    count: u32,
}

fn load() -> Data {
    Data { count: COUNT.load(Ordering::Relaxed) }
}

#[action]
fn bump() {
    COUNT.fetch_add(1, Ordering::Relaxed);
}
