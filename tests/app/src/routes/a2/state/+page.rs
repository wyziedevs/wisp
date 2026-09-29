use std::sync::atomic::{AtomicU32, Ordering};
use wisp::prelude::*;

static COUNT: AtomicU32 = AtomicU32::new(0);

pub struct Data {
    pub count: u32,
}

pub fn load() -> Data {
    Data {
        count: COUNT.load(Ordering::Relaxed),
    }
}

#[action]
pub fn bump() {
    COUNT.fetch_add(1, Ordering::Relaxed);
}
