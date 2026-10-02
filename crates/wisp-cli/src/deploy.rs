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
/// they use into `out` (see `wisp::export`), then adds `static/`.
pub fn static_site(root: &Path, exe: &Path, out: &Path) -> Result<(), String> {
    check_out(root, out)?;
    term::step(&format!("Exporting to {}", out.display()));
    let mut child = Command::new(exe)
        .env("WISP_EXPORT", out)
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
    Ok(())
}

pub fn copy_dir(from: &Path, to: &Path) -> Result<usize, String> {
    let io = |p: &Path, e: std::io::Error| format!("{}: {e}", p.display());
    std::fs::create_dir_all(to).map_err(|e| io(to, e))?;
    let mut n = 0;
    for entry in std::fs::read_dir(from).map_err(|e| io(from, e))? {
        let entry = entry.map_err(|e| io(from, e))?;
        let target = to.join(entry.file_name());
        if entry.path().is_dir() {
            n += copy_dir(&entry.path(), &target)?;
        } else {
            std::fs::copy(entry.path(), &target).map_err(|e| io(&target, e))?;
            n += 1;
        }
    }
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

/// `.wisp/app.css` is the built CSS, which the image's build needs.
const DOCKERIGNORE: &str = "target
.git
node_modules
.wisp/*
!.wisp/app.css
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
}
