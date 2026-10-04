---
use std::sync::atomic::{AtomicU64, Ordering};

const CACHE: u32 = 60;
static RENDERS: AtomicU64 = AtomicU64::new(0);

let n = RENDERS.fetch_add(1, Ordering::Relaxed);
if cx.query("cookie").is_some() {
    cx.set_cookie("seen", n);
}
---

<p>render {n}</p>
