//! A project on disk for `wisp_build::check`, and the ways to use one.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

static COUNT: AtomicUsize = AtomicUsize::new(0);

/// A directory of `(path, contents)` files, removed when dropped.
pub struct Project(PathBuf);

impl Project {
    pub fn new(files: &[(&str, &str)]) -> Project {
        let n = COUNT.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!("wisp-build-it-{}-{n}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("src")).unwrap();
        for (path, contents) in files {
            let file = root.join(path);
            fs::create_dir_all(file.parent().unwrap()).unwrap();
            fs::write(file, contents).unwrap();
        }
        Project(root)
    }

    pub fn root(&self) -> &Path {
        &self.0
    }
}

impl Drop for Project {
    fn drop(&mut self) {
        // On Windows a file still in use (by a scan, say) holds its folder.
        for _ in 0..40 {
            if fs::remove_dir_all(&self.0).is_ok() || !self.0.exists() {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(25));
        }
    }
}

/// A project that must pass `check`.
pub fn passes(files: &[(&str, &str)]) {
    if let Err(e) = wisp_build::check(Project::new(files).root()) {
        panic!("check failed: {e}");
    }
}

/// A name, a project's files, and what its error must contain.
pub type Case<'a> = (&'a str, &'a [(&'a str, &'a str)], &'a [&'a str]);

/// Projects that must fail `check`: each case is a name, its files, and
/// what its error must contain (a `file:line` and the key phrase). Every
/// case is tried before the test fails, and the report lists all that were off.
pub fn fails(cases: &[Case]) {
    let bad: Vec<String> = cases
        .iter()
        .filter_map(|(name, files, want)| refused(name, files, want))
        .collect();
    assert!(bad.is_empty(), "{}", bad.join("\n\n"));
}

/// Page sources that must fail `check`: each case is a name, the page's
/// source, and what its error must contain.
pub fn page_fails(cases: &[(&str, &str, &[&str])]) {
    let bad: Vec<String> = cases
        .iter()
        .filter_map(|(name, src, want)| refused(name, &[("src/routes/+page.wisp", src)], want))
        .collect();
    assert!(bad.is_empty(), "{}", bad.join("\n\n"));
}

/// What is wrong with how `check` answered, if anything.
fn refused(name: &str, files: &[(&str, &str)], want: &[&str]) -> Option<String> {
    let Err(e) = wisp_build::check(Project::new(files).root()) else {
        return Some(format!(
            "{name}: check passed, but should fail with {want:?}"
        ));
    };
    let missing: Vec<&&str> = want.iter().filter(|w| !e.contains(**w)).collect();
    (!missing.is_empty()).then(|| format!("{name}: {missing:?} not in the error:\n{e}"))
}
