---
use std::sync::atomic::{AtomicU64, Ordering};

const CACHE_PUBLIC: u32 = 60;
static RENDERS: AtomicU64 = AtomicU64::new(0);
let n = RENDERS.fetch_add(1, Ordering::Relaxed);
---
<main data-grp="{n}">{@render children()}</main>
