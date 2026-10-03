---
use std::sync::atomic::{AtomicU64, Ordering};

const PRERENDER: bool = true;
static RENDERS: AtomicU64 = AtomicU64::new(0);

let n = RENDERS.fetch_add(1, Ordering::Relaxed);
---
<p>render {n}</p>
