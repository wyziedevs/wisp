//! The commit a git checkout is at, read from its files (`HEAD`, the ref it
//! names, `packed-refs`) so that no process is spawned. Shared with build.rs.

use std::fs;
use std::path::{Path, PathBuf};

/// The commit `repo` is at, and the files that say so (to watch for change).
/// `None` when `repo` is not a checkout or anything about it is unexpected.
pub fn read(repo: &Path) -> Option<(String, Vec<PathBuf>)> {
    let (git, common) = dirs(repo)?;
    let head_file = git.join("HEAD");
    let head = fs::read_to_string(&head_file).ok()?;
    let mut files = vec![head_file];
    let Some(name) = head.trim().strip_prefix("ref:").map(str::trim) else {
        return Some((sha(head.trim())?, files));
    };
    let loose = common.join(name);
    if let Ok(text) = fs::read_to_string(&loose) {
        files.push(loose);
        return Some((sha(text.trim())?, files));
    }
    let packed = common.join("packed-refs");
    let text = fs::read_to_string(&packed).ok()?;
    files.push(packed);
    let line = text
        .lines()
        .find_map(|l| l.trim_end().strip_suffix(name)?.strip_suffix(' '))?;
    Some((sha(line)?, files))
}

/// `.git`, and where the refs are: the same, except in a linked worktree
/// (where `.git` is a file that points to the former, and `commondir` to the
/// latter).
fn dirs(repo: &Path) -> Option<(PathBuf, PathBuf)> {
    let dot = repo.join(".git");
    if dot.is_dir() {
        return Some((dot.clone(), dot));
    }
    let text = fs::read_to_string(&dot).ok()?;
    let git = repo.join(text.trim().strip_prefix("gitdir:")?.trim());
    let common = match fs::read_to_string(git.join("commondir")) {
        Ok(c) => git.join(c.trim()),
        Err(_) => git.clone(),
    };
    Some((git, common))
}

fn sha(s: &str) -> Option<String> {
    (s.len() >= 40 && s.bytes().all(|b| b.is_ascii_hexdigit())).then(|| s.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("wisp-git-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn reads_a_loose_ref_then_a_packed_one() {
        let repo = temp("loose");
        let git = repo.join(".git");
        fs::create_dir_all(git.join("refs/heads")).unwrap();
        fs::write(git.join("HEAD"), "ref: refs/heads/main\r\n").unwrap();
        fs::write(
            git.join("packed-refs"),
            format!("# pack-refs\n{A} refs/heads/main\n"),
        )
        .unwrap();
        assert_eq!(read(&repo).unwrap().0, A);
        fs::write(git.join("refs/heads/main"), format!("{B}\n")).unwrap();
        let (commit, files) = read(&repo).unwrap();
        assert_eq!((commit.as_str(), files.len()), (B, 2));
        fs::remove_dir_all(&repo).unwrap();
    }

    #[test]
    fn reads_a_detached_head_and_refuses_junk() {
        let repo = temp("detached");
        let git = repo.join(".git");
        fs::create_dir_all(&git).unwrap();
        fs::write(git.join("HEAD"), format!("{A}\n")).unwrap();
        assert_eq!(read(&repo).unwrap().0, A);
        fs::write(git.join("HEAD"), "ref: refs/heads/gone\n").unwrap();
        assert_eq!(read(&repo), None);
        fs::write(git.join("HEAD"), "not a commit\n").unwrap();
        assert_eq!(read(&repo), None);
        assert_eq!(read(&repo.join("nowhere")), None);
        fs::remove_dir_all(&repo).unwrap();
    }

    #[test]
    fn reads_a_linked_worktree() {
        let main = temp("main");
        let tree = temp("tree");
        let common = main.join(".git");
        let git = common.join("worktrees/t");
        fs::create_dir_all(common.join("refs/heads")).unwrap();
        fs::create_dir_all(&git).unwrap();
        fs::write(git.join("HEAD"), "ref: refs/heads/work\n").unwrap();
        fs::write(git.join("commondir"), "../..\n").unwrap();
        fs::write(common.join("refs/heads/work"), format!("{B}\n")).unwrap();
        fs::write(tree.join(".git"), format!("gitdir: {}\n", git.display())).unwrap();
        assert_eq!(read(&tree).unwrap().0, B);
        fs::remove_dir_all(&main).unwrap();
        fs::remove_dir_all(&tree).unwrap();
    }
}
