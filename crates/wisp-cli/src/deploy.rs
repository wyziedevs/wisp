//! `wisp build --static` and `wisp build --docker`: ways to host the app
//! other than running its one binary.

use crate::term;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// Where `wisp build --static` (and `--target`, in a folder per host)
/// writes, without `--out`.
pub const DEFAULT_OUT: &str = "dist";

/// Refuses an `--out` that is the app's own folder, one it is inside, or
/// inside `src/` or `static/`: exporting there would write over the app's
/// files, or copy `static/` into itself.
pub fn check_out(root: &Path, out: &Path) -> Result<(), String> {
    let (root, out) = (resolved(root), resolved(out));
    if root.starts_with(&out)
        || out.starts_with(root.join("src"))
        || out.starts_with(root.join("static"))
    {
        return Err(format!(
            "{} is part of the app.\nPick another folder with --out, like dist.",
            out.display()
        ));
    }
    Ok(())
}

/// An absolute path with symlinks and `..` resolved as far as it exists.
fn resolved(path: &Path) -> PathBuf {
    let abs = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    let mut tail = Vec::new();
    let mut at = abs.as_path();
    loop {
        if let Ok(found) = at.canonicalize() {
            return tail.iter().rev().fold(found, |p, name| p.join(name));
        }
        match (at.parent(), at.file_name()) {
            (Some(parent), Some(name)) => {
                tail.push(name);
                at = parent;
            }
            _ => return abs,
        }
    }
}

/// Runs the built app in export mode, which writes its pages and the files
/// they use into `out` (see `wisp::export`), then adds `static/`. `spa`:
/// with the fallback, `index.html`, that draws the pages the browser draws.
pub fn static_site(root: &Path, exe: &Path, out: &Path, spa: bool) -> Result<(), String> {
    check_out(root, out)?;
    term::step(&format!("Exporting to {}", out.display()));
    let mut child = Command::new(exe)
        .env("WISP_EXPORT", out)
        .env("WISP_SPA", if spa { "1" } else { "" })
        .current_dir(root)
        .stdout(Stdio::piped())
        .spawn()
        .map_err(|e| format!("Could not run {}: {e}.", exe.display()))?;
    let mut files = 0;
    term::each_line(
        child.stdout.take().expect("stdout is piped"),
        |line| match line.split_once(' ') {
            Some(("wrote", _)) => files += 1,
            Some(("warn", msg)) => term::warn(msg),
            _ => println!("{line}"),
        },
    );
    if !child.wait().is_ok_and(|s| s.success()) {
        return Err("The export failed.\nThe app's message is above.".into());
    }
    let static_dir = root.join("static");
    if static_dir.is_dir() {
        files += copy_dir(&static_dir, out)?;
    }
    term::done(&format!("Exported {files} files to {}", out.display()));
    println!(
        "    Serve that folder from any static host: GitHub Pages, Netlify, Cloudflare Pages, S3."
    );
    if spa {
        println!(
            "    Have it answer a missing path with index.html (Netlify: `/* /index.html 200` in _redirects)."
        );
    }
    Ok(())
}

/// Where `wisp build` prerenders pages, for the build after it to embed.
const PRERENDERED: &str = ".wisp/prerender";

/// Whether a page of the app says `const PRERENDER: bool = true;`: a look
/// at the text, which the build checks.
pub fn prerenders(root: &Path) -> bool {
    fn any(dir: &Path, depth: u32) -> bool {
        (std::fs::read_dir(dir).into_iter().flatten().flatten()).any(|e| {
            let p = e.path();
            if p.is_dir() {
                return depth < 32 && any(&p, depth + 1);
            }
            let page = matches!(
                p.file_name().and_then(|n| n.to_str()),
                Some("+page.wisp" | "+page.rs")
            );
            page && std::fs::read_to_string(&p).is_ok_and(|t| t.contains("PRERENDER: bool = true"))
        })
    }
    any(&root.join("src").join("routes"), 0)
}

/// Runs the built app to render its prerendered pages into
/// `.wisp/prerender` (see `wisp::export::prerender`), then builds it again
/// with them inside (`WISP_PRERENDERED`), with `env` as the first build
/// had.
pub fn prerender(root: &Path, exe: &Path, env: &[(&str, &str)]) -> Result<(), String> {
    let dir =
        std::path::absolute(root.join(PRERENDERED)).map_err(|e| format!("{PRERENDERED}: {e}"))?;
    let _ = std::fs::remove_dir_all(&dir);
    term::step("Prerendering");
    let mut child = Command::new(exe)
        .env("WISP_PRERENDER", &dir)
        .current_dir(root)
        .stdout(Stdio::piped())
        .spawn()
        .map_err(|e| format!("Could not run {}: {e}.", exe.display()))?;
    let mut pages = 0;
    term::each_line(
        child.stdout.take().expect("stdout is piped"),
        |line| match line.split_once(' ') {
            Some(("wrote", _)) => pages += 1,
            Some(("warn", msg)) => term::warn(msg),
            _ => println!("{line}"),
        },
    );
    if !child.wait().is_ok_and(|s| s.success()) {
        return Err("Prerendering failed.\nThe app's message is above.".into());
    }
    let dir = dir.to_string_lossy();
    let mut env = env.to_vec();
    env.push(("WISP_PRERENDERED", &dir));
    let b = crate::cargo::build_for(root, true, false, &[], &env);
    if !b.ok {
        return Err(
            "The build with the prerendered pages failed.\nThe compiler's errors are above.".into(),
        );
    }
    let s = if pages == 1 { "" } else { "s" };
    term::done(&format!("Prerendered {pages} page{s} into the binary"));
    Ok(())
}

/// Copies the folder `from` into `to`, following symlinks, and returns
/// the files copied. A symlinked folder that leads back into a folder being
/// copied, or folders nested past 64 deep, is an error rather than a hang.
pub fn copy_dir(from: &Path, to: &Path) -> Result<usize, String> {
    copy_tree(from, to, &mut Vec::new())
}

/// `copy_dir`, with `open` the real paths of the folders being copied.
fn copy_tree(from: &Path, to: &Path, open: &mut Vec<std::path::PathBuf>) -> Result<usize, String> {
    let io = |p: &Path, e: std::io::Error| format!("{}: {e}", p.display());
    let real = std::fs::canonicalize(from).map_err(|e| io(from, e))?;
    if open.contains(&real) {
        return Err(format!(
            "{}: a symlink loops back to a folder it is in.
Remove the link or point it elsewhere.",
            from.display()
        ));
    }
    if open.len() >= 64 {
        return Err(format!(
            "{}: folders nest more than 64 deep.",
            from.display()
        ));
    }
    open.push(real);
    std::fs::create_dir_all(to).map_err(|e| io(to, e))?;
    let mut n = 0;
    for entry in std::fs::read_dir(from).map_err(|e| io(from, e))? {
        let entry = entry.map_err(|e| io(from, e))?;
        let target = to.join(entry.file_name());
        if entry.path().is_dir() {
            n += copy_tree(&entry.path(), &target, open)?;
        } else {
            std::fs::copy(entry.path(), &target).map_err(|e| io(&target, e))?;
            n += 1;
        }
    }
    open.pop();
    Ok(n)
}

/// Writes a `Dockerfile` and `.dockerignore`. Without `force`, files that
/// exist are left alone and it says so.
pub fn docker(root: &Path, package: &str, force: bool) -> Result<(), String> {
    let files = [
        ("Dockerfile", dockerfile(package)),
        (".dockerignore", DOCKERIGNORE.to_string()),
    ];
    let taken: Vec<&str> = files
        .iter()
        .map(|f| f.0)
        .filter(|name| root.join(name).exists())
        .collect();
    if !taken.is_empty() && !force {
        return Err(format!(
            "{} already exist{}.\nRun wisp build --docker --force to replace {}.",
            taken.join(" and "),
            if taken.len() == 1 { "s" } else { "" },
            if taken.len() == 1 { "it" } else { "them" }
        ));
    }
    for (name, text) in files {
        std::fs::write(root.join(name), text).map_err(|e| format!("{name}: {e}"))?;
        term::done(&format!("Wrote {name}"));
    }
    println!(
        "    Build it with docker build -t {package} . and run it with docker run -p 3000:3000 -e WISP_SECRET=... {package}"
    );
    println!(
        "    Fly.io, Railway, Render, Cloud Run and Azure Container Apps build a Dockerfile as it is."
    );
    Ok(())
}

fn dockerfile(package: &str) -> String {
    format!(
        "# Written by wisp build --docker. Set WISP_SECRET (32 or more random characters) when you run it.
FROM rust:slim AS build
WORKDIR /app
COPY . .
RUN cargo build --release && cp target/release/{package} /server

FROM debian:stable-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates && rm -rf /var/lib/apt/lists/* \\
 && mkdir /data && chown nobody /data
COPY --from=build /server /usr/local/bin/server
# Saved tables (#[derive(Rest)], Table::saved) live in /data: mount a volume there.
ENV HOST=0.0.0.0 PORT=3000 WISP_DATA=/data
VOLUME /data
EXPOSE 3000
USER nobody
CMD [\"server\"]
"
    )
}

/// `.wisp/app.css` is the built CSS, `.wisp/npm` the npm packages and
/// `.wisp/img` the images' WebP widths, which the image's build needs.
const DOCKERIGNORE: &str = "target
.git
node_modules
.wisp/*
!.wisp/app.css
!.wisp/npm
!.wisp/img
dist
data
Dockerfile
.dockerignore
";

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("wisp-deploy-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn docker_files_are_not_overwritten_without_force() {
        let dir = temp("docker");
        docker(&dir, "my-app", false).unwrap();
        let text = std::fs::read_to_string(dir.join("Dockerfile")).unwrap();
        assert!(
            text.contains("cp target/release/my-app /server")
                && text.contains("HOST=0.0.0.0 PORT=3000")
        );
        std::fs::write(dir.join("Dockerfile"), "mine").unwrap();
        let err = docker(&dir, "my-app", false).unwrap_err();
        assert!(
            err.contains("--force")
                && err.starts_with("Dockerfile and .dockerignore already exist"),
            "{err}"
        );
        assert_eq!(
            std::fs::read_to_string(dir.join("Dockerfile")).unwrap(),
            "mine"
        );
        docker(&dir, "my-app", true).unwrap();
        assert!(
            std::fs::read_to_string(dir.join("Dockerfile"))
                .unwrap()
                .contains("FROM rust:slim")
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn out_stays_away_from_the_app() {
        let root = temp("out");
        std::fs::create_dir_all(root.join("static")).unwrap();
        for bad in [
            ".",
            "..",
            "../..",
            "src",
            "static",
            "static/public",
            "./static/../static/x",
        ] {
            let err = check_out(&root, &root.join(bad)).unwrap_err();
            assert!(err.contains("part of the app"), "{bad}: {err}");
        }
        for fine in ["dist", "dist/node", "../elsewhere", "statics"] {
            check_out(&root, &root.join(fine)).unwrap();
        }
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn copies_folders() {
        let (from, to) = (temp("from"), temp("to"));
        std::fs::create_dir_all(from.join("img")).unwrap();
        std::fs::write(from.join("a.txt"), "a").unwrap();
        std::fs::write(from.join("img/b.txt"), "b").unwrap();
        assert_eq!(copy_dir(&from, &to).unwrap(), 2);
        assert_eq!(std::fs::read_to_string(to.join("img/b.txt")).unwrap(), "b");
        std::fs::remove_dir_all(&from).unwrap();
        std::fs::remove_dir_all(&to).unwrap();
    }

    #[test]
    fn a_symlink_loop_is_an_error_not_a_hang() {
        let (from, to) = (temp("loop-from"), temp("loop-to"));
        std::fs::create_dir_all(from.join("a")).unwrap();
        std::fs::write(from.join("a/x.txt"), "x").unwrap();
        #[cfg(unix)]
        let made = std::os::unix::fs::symlink(&from, from.join("a/up"));
        #[cfg(windows)]
        let made = std::os::windows::fs::symlink_dir(&from, from.join("a/up"));
        if made.is_err() {
            // Windows without the right to make symlinks: nothing to test.
            std::fs::remove_dir_all(&from).unwrap();
            return;
        }
        let e = copy_dir(&from, &to).unwrap_err();
        assert!(e.contains("loops back"), "{e}");
        std::fs::remove_dir_all(&from).unwrap();
        let _ = std::fs::remove_dir_all(&to);
    }
}
