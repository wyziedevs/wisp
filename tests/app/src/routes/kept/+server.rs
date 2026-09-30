// `CACHE_PUBLIC`: what `get` answers is kept for every request, one with
// a cookie or credentials too.

use std::sync::atomic::{AtomicU64, Ordering};

const CACHE_PUBLIC: u32 = 60;
static CALLS: AtomicU64 = AtomicU64::new(0);

fn get() -> String {
    format!("call {}", CALLS.fetch_add(1, Ordering::Relaxed))
}
