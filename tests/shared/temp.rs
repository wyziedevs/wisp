//! A temp path for a test, shared by the tests of `wisp` and of the test app.

use std::ops::Deref;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

/// A path under the system's temp folder that nothing is at yet, and
/// nothing is at again once this is dropped (a folder, or a file).
pub struct Temp(PathBuf);

impl Temp {
    pub fn new(name: &str) -> Temp {
        static N: AtomicUsize = AtomicUsize::new(0);
        let n = N.fetch_add(1, Ordering::Relaxed);
        let path =
            std::env::temp_dir().join(format!("wisp-test-{}-{n}-{name}", std::process::id()));
        remove(&path);
        Temp(path)
    }
}

impl Deref for Temp {
    type Target = Path;
    fn deref(&self) -> &Path {
        &self.0
    }
}

impl Drop for Temp {
    fn drop(&mut self) {
        remove(&self.0);
    }
}

/// Removes what is at `path`. On Windows a process that is still exiting
/// holds its files, so it tries for a while.
fn remove(path: &Path) {
    for _ in 0..40 {
        let done = if path.is_dir() {
            std::fs::remove_dir_all(path)
        } else {
            std::fs::remove_file(path)
        };
        if done.is_ok() || !path.exists() {
            return;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
}
