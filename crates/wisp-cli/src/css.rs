//! The CSS step, for an app that opts in. `src/app.scss` is built by Sass,
//! or a `src/app.css` that imports Tailwind by Tailwind, each a standalone
//! binary (no Node), into `.wisp/app.css`; with a `postcss.config.*`,
//! PostCSS (by npx: it needs Node) runs on the result, or on a plain
//! `src/app.css`. Otherwise `src/app.css` is served as written and there
//! is nothing to run.

use crate::{net, term};
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use wisp_shared::sha256::{hex, sha256};

pub const TAILWIND_VERSION: &str = "v4.3.3";

/// Release asset and its SHA-256, from the GitHub release metadata.
const TAILWIND_ASSETS: &[(&str, &str, &str, &str)] = &[
    // (os, arch, asset, sha256)
    (
        "windows",
        "x86_64",
        "tailwindcss-windows-x64.exe",
        "e0e260ce048014e9268f6237ff18f8ccf02cef521cbd0ae04e82c2cdf7aa3955",
    ),
    (
        "linux",
        "x86_64",
        "tailwindcss-linux-x64",
        "dc61b3ac6b8c9ca874c0cc4c57b2409791a64c5540404ca5f5367360babc313a",
    ),
    (
        "linux",
        "aarch64",
        "tailwindcss-linux-arm64",
        "55fd0b241214eff3de1e8ee4f22796662f2d2e7a49bcfca7477cfd0bac398195",
    ),
    (
        "macos",
        "aarch64",
        "tailwindcss-macos-arm64",
        "cdf646702987a743464dff4d9c60fd4480d1c1e73dd819a9a67f1078815dce9d",
    ),
    (
        "macos",
        "x86_64",
        "tailwindcss-macos-x64",
        "7922e0953f2110c05976e3bf58f14e643d90427575e766b7d433f5f80cbee7e1",
    ),
];

pub const SASS_VERSION: &str = "1.105.1";

/// Dart Sass's release archives (a Dart runtime and Sass's snapshot), the
/// same way.
const SASS_ASSETS: &[(&str, &str, &str, &str)] = &[
    (
        "windows",
        "x86_64",
        "windows-x64.zip",
        "3f76ae65dd7b494cc25cd2257f3be608e074b2d33b7328d9ba565bd60ba54f7e",
    ),
    (
        "linux",
        "x86_64",
        "linux-x64.tar.gz",
        "9046fdd4a31a524a0020298c55d9e155d4c22363ef268e8992574335a72c6a6f",
    ),
    (
        "linux",
        "aarch64",
        "linux-arm64.tar.gz",
        "7f53a2a77ef4aaf24a9f2f8c826759ed93b38954430a2e32130921a2e16a4a1c",
    ),
    (
        "macos",
        "aarch64",
        "macos-arm64.tar.gz",
        "4453e66a326f350821622912d99115eefe6a636838a10a279e341254ef00594d",
    ),
    (
        "macos",
        "x86_64",
        "macos-x64.tar.gz",
        "f40e107223cebaaf3dbd6c6ed5849dbb791efd008f80a58df2746e5fa54cf1eb",
    ),
];

const POSTCSS_CONFIGS: [&str; 6] = [
    "postcss.config.js",
    "postcss.config.cjs",
    "postcss.config.mjs",
    "postcss.config.ts",
    "postcss.config.json",
    ".postcssrc.json",
];

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Tool {
    Tailwind,
    Sass,
}

/// What builds the app's CSS.
#[derive(Debug, PartialEq)]
pub struct Css {
    pub tool: Option<Tool>,
    pub postcss: bool,
}

impl Css {
    /// `src/app.css` is served as written.
    pub fn plain(&self) -> bool {
        self.tool.is_none() && !self.postcss
    }

    /// Where the tool writes: PostCSS's input when it runs after.
    fn tool_out(&self) -> &'static str {
        if self.postcss {
            ".wisp/pre.css"
        } else {
            ".wisp/app.css"
        }
    }

    fn postcss_in(&self) -> &'static str {
        if self.tool.is_some() {
            ".wisp/pre.css"
        } else {
            "src/app.css"
        }
    }
}

pub fn detect(root: &Path) -> Css {
    let src = root.join("src");
    let css = src.join("app.css");
    let tool = if src.join("app.scss").exists() {
        Some(Tool::Sass)
    } else if wisp_build::uses_tailwind(&wisp_build::read_source(&css).unwrap_or_default()) {
        Some(Tool::Tailwind)
    } else {
        None
    };
    let postcss =
        (tool.is_some() || css.exists()) && POSTCSS_CONFIGS.iter().any(|c| root.join(c).exists());
    Css { tool, postcss }
}

/// Kills the watcher when dropped. Its stdin is a pipe we hold open: when the
/// CLI dies for any reason, the pipe closes and Tailwind exits by itself
/// (Sass and PostCSS go with the terminal's Ctrl+C).
pub struct Watcher(Child);

impl Drop for Watcher {
    fn drop(&mut self) {
        drop(self.0.stdin.take());
        // npx on Windows is a .cmd: its node is a grandchild.
        if cfg!(windows) {
            let _ = Command::new("taskkill")
                .args(["/T", "/F", "/PID", &self.0.id().to_string()])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
        }
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// The watchers that rebuild `.wisp/app.css` on every change, for `wisp dev`.
pub fn watch(root: &Path) -> Result<Vec<Watcher>, String> {
    let css = detect(root);
    if css.plain() {
        // A stale build would shadow the plain src/app.css.
        let _ = fs::remove_file(root.join(".wisp").join("app.css"));
        return Ok(Vec::new());
    }
    make_wisp_dir(root)?;
    let mut out = Vec::new();
    if let Some(tool) = css.tool {
        if css.postcss {
            // PostCSS watches the tool's output, which must be there first.
            run_tool(root, tool, &css, false)?;
        }
        let (mut cmd, name) = command(tool, &css, true)?;
        let child = spawn(cmd.current_dir(root), name)?;
        out.push(Watcher(pipe_errors(child, name, |line| {
            line.is_empty() || line.starts_with("≈ tailwindcss") || line.starts_with("Done in ")
        })));
    }
    if css.postcss {
        let mut cmd = postcss(&css, true);
        let child = spawn(cmd.current_dir(root), "PostCSS")?;
        out.push(Watcher(pipe_errors(child, "PostCSS", |line| {
            line.is_empty()
                || line.starts_with("Processing ")
                || line.starts_with("Finished ")
                || line.starts_with("Waiting for file changes")
        })));
    }
    Ok(out)
}

/// One minified build, for `wisp build`.
pub fn build(root: &Path) -> Result<(), String> {
    let css = detect(root);
    if css.plain() {
        // A stale build would shadow the plain src/app.css.
        let _ = fs::remove_file(root.join(".wisp").join("app.css"));
        return Ok(());
    }
    make_wisp_dir(root)?;
    if let Some(tool) = css.tool {
        run_tool(root, tool, &css, true)?;
    }
    if css.postcss {
        let status = postcss(&css, false)
            .current_dir(root)
            .status()
            .map_err(|e| no_node(&e))?;
        if !status.success() {
            return Err("PostCSS could not build the CSS.\nIts errors are above. Is it installed? npm install -D postcss postcss-cli".into());
        }
    }
    Ok(())
}

/// One build by the tool: minified for release, else as `wisp dev` writes.
fn run_tool(root: &Path, tool: Tool, css: &Css, release: bool) -> Result<(), String> {
    let (mut cmd, name) = command(tool, css, false)?;
    if release {
        cmd.arg(match tool {
            Tool::Tailwind => "--minify",
            Tool::Sass => "--style=compressed",
        });
    }
    let status = cmd
        .current_dir(root)
        .status()
        .map_err(|e| format!("Could not run {name}: {e}."))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!(
            "{name} could not build the CSS.\nIts errors are above."
        ))
    }
}

/// The tool's command, with its input and output, and its name.
fn command(tool: Tool, css: &Css, watch: bool) -> Result<(Command, &'static str), String> {
    let out = css.tool_out();
    let (mut cmd, name) = match tool {
        Tool::Tailwind => {
            let mut cmd = Command::new(tailwind()?);
            cmd.args(["-i", "src/app.css", "-o", out]);
            (cmd, "Tailwind")
        }
        Tool::Sass => {
            let (bin, pre) = sass()?;
            let mut cmd = Command::new(bin);
            cmd.args(pre).args(["--no-source-map", "src/app.scss", out]);
            (cmd, "Sass")
        }
    };
    if watch {
        cmd.arg("--watch");
    }
    Ok((cmd, name))
}

/// `npx postcss`, from PostCSS's input to `.wisp/app.css`. `--no-install`:
/// the app's own PostCSS, never one npx downloads.
fn postcss(css: &Css, watch: bool) -> Command {
    let mut cmd = Command::new(if cfg!(windows) { "npx.cmd" } else { "npx" });
    cmd.args([
        "--no-install",
        "postcss",
        css.postcss_in(),
        "-o",
        ".wisp/app.css",
    ]);
    if watch {
        cmd.arg("--watch");
    }
    cmd
}

fn no_node(e: &std::io::Error) -> String {
    format!(
        "Could not run npx for PostCSS: {e}.\nPostCSS needs Node: install it (with npm), then npm install -D postcss postcss-cli. Or remove postcss.config.*."
    )
}

fn spawn(cmd: &mut Command, name: &str) -> Result<Child, String> {
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| match name {
            "PostCSS" => no_node(&e),
            _ => format!("Could not start {name}: {e}."),
        })
}

/// The tool reports every build on stderr; `wisp dev` already says when
/// the styles change, so only its errors (and lines `quiet` keeps) get
/// through.
fn pipe_errors(mut child: Child, name: &'static str, quiet: fn(&str) -> bool) -> Child {
    let stderr = child.stderr.take().expect("stderr is piped");
    std::thread::spawn(move || {
        term::each_line(stderr, |line| {
            if let Some(e) = line.strip_prefix("Error: ") {
                term::failed(&format!("{name} could not build the CSS.\n{e}"));
            } else if !quiet(line) {
                eprintln!("{line}");
            }
        });
    });
    child
}

/// The tools write into `.wisp/`, which a fresh checkout lacks.
fn make_wisp_dir(root: &Path) -> Result<(), String> {
    let dir = root.join(".wisp");
    fs::create_dir_all(&dir).map_err(|e| format!("Could not create {}: {e}.", dir.display()))
}

/// Downloads Tailwind now rather than on the first `wisp dev`.
pub fn install() -> Result<(), String> {
    tailwind().map(drop)
}

/// `$WISP_TAILWIND`, else the pinned version in `~/.wisp/bin`, downloaded
/// and verified on first use.
fn tailwind() -> Result<PathBuf, String> {
    if let Some(p) = std::env::var_os("WISP_TAILWIND") {
        return Ok(PathBuf::from(p));
    }
    let set = "set WISP_TAILWIND to a tailwindcss binary";
    let (asset, sha) = asset(TAILWIND_ASSETS, "Tailwind", set)?;
    let bin = bin_dir()?.join(format!(
        "tailwindcss-{TAILWIND_VERSION}{}",
        std::env::consts::EXE_SUFFIX
    ));
    if bin.exists() {
        return Ok(bin);
    }
    let url = format!(
        "https://github.com/tailwindlabs/tailwindcss/releases/download/{TAILWIND_VERSION}/{asset}"
    );
    let file = download(
        &format!("Tailwind {TAILWIND_VERSION}"),
        &url,
        sha,
        &bin,
        set,
    )?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&file, fs::Permissions::from_mode(0o755))
            .map_err(|e| format!("{}: {e}.", file.display()))?;
    }
    place(&file, &bin, "Tailwind")?;
    Ok(bin)
}

/// `$WISP_SASS` (a sass binary), else the pinned Dart Sass in
/// `~/.wisp/bin`, downloaded, verified and unpacked on first use: its Dart
/// runtime, and the snapshot it runs.
fn sass() -> Result<(PathBuf, Vec<PathBuf>), String> {
    if let Some(p) = std::env::var_os("WISP_SASS") {
        return Ok((PathBuf::from(p), Vec::new()));
    }
    let set = "set WISP_SASS to a sass binary";
    let (asset, sha) = asset(SASS_ASSETS, "Dart Sass", set)?;
    let dir = bin_dir()?.join(format!("dart-sass-{SASS_VERSION}"));
    let run = |dir: &Path| {
        let src = dir.join("src");
        let dart = src.join(format!("dart{}", std::env::consts::EXE_SUFFIX));
        (dart, vec![src.join("sass.snapshot")])
    };
    if dir.exists() {
        return Ok(run(&dir));
    }
    let url = format!(
        "https://github.com/sass/dart-sass/releases/download/{SASS_VERSION}/dart-sass-{SASS_VERSION}-{asset}"
    );
    let archive = download(&format!("Sass {SASS_VERSION}"), &url, sha, &dir, set)?;
    // Unpacked beside it, then moved into place whole.
    let mut tmp = OsString::from(dir.as_os_str());
    tmp.push(format!(".{}.tmp", std::process::id()));
    let tmp = PathBuf::from(tmp);
    let unpacked = fs::create_dir_all(&tmp)
        .map_err(|e| format!("Could not create {}: {e}.", tmp.display()))
        .and_then(|()| untar(&archive, &tmp))
        .and_then(|()| place(&tmp.join("dart-sass"), &dir, "Sass"));
    let _ = fs::remove_file(&archive);
    let _ = fs::remove_dir_all(&tmp);
    unpacked?;
    Ok(run(&dir))
}

/// tar unpacks both: Windows 10+ ships bsdtar, which reads zip files too.
fn untar(archive: &Path, into: &Path) -> Result<(), String> {
    let system =
        std::env::var_os("SystemRoot").map(|r| PathBuf::from(r).join("System32").join("tar.exe"));
    let tar = system
        .filter(|t| t.exists())
        .unwrap_or_else(|| "tar".into());
    let status = Command::new(&tar)
        .arg("-xf")
        .arg(archive)
        .arg("-C")
        .arg(into)
        .status()
        .map_err(|e| format!("Could not run tar to unpack Sass: {e}.\nInstall tar, or set WISP_SASS to a sass binary."))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!(
            "Could not unpack {}.\nSet WISP_SASS to a sass binary.",
            archive.display()
        ))
    }
}

/// This machine's asset of a tool and its SHA-256.
fn asset(
    assets: &'static [(&str, &str, &'static str, &'static str)],
    name: &str,
    set: &str,
) -> Result<(&'static str, &'static str), String> {
    let (os, arch) = (std::env::consts::OS, std::env::consts::ARCH);
    assets
        .iter()
        .find(|(o, a, _, _)| *o == os && *a == arch)
        .map(|&(_, _, asset, sha)| (asset, sha))
        .ok_or_else(|| format!("{name} has no build for {os}-{arch}.\nInstead, {set}."))
}

/// `~/.wisp/bin`, made if missing.
fn bin_dir() -> Result<PathBuf, String> {
    let home = std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .ok_or("Could not find the home folder.\nSet HOME or USERPROFILE.")?;
    let dir = PathBuf::from(home).join(".wisp").join("bin");
    fs::create_dir_all(&dir).map_err(|e| format!("Could not create {}: {e}.", dir.display()))?;
    Ok(dir)
}

/// Downloads `url` into a file beside `dest` and checks it is the file
/// expected. The process id keeps two `wisp` commands from writing one file.
fn download(what: &str, url: &str, sha: &str, dest: &Path, set: &str) -> Result<PathBuf, String> {
    let mut partial = OsString::from(dest.as_os_str());
    partial.push(format!(".{}.download", std::process::id()));
    let partial = PathBuf::from(partial);
    term::step(&format!(
        "Downloading {what} from {url}. This happens once."
    ));
    if let Err(e) = net::fetch(url, Some(&partial)) {
        let _ = fs::remove_file(&partial);
        return Err(format!(
            "Could not download {what}: {}.\nTried {url}. Check the network, or {set}.",
            e.text()
        ));
    }
    let bytes = fs::read(&partial).map_err(|e| format!("{}: {e}.", partial.display()))?;
    let got = hex(&sha256(&[&bytes]));
    if got != sha {
        let _ = fs::remove_file(&partial);
        return Err(format!(
            "The {what} download is not the expected file, so it was not used.\nExpected SHA-256 {sha}, got {got}."
        ));
    }
    Ok(partial)
}

/// Moves a finished download into place. Another `wisp` may have finished
/// first; its copy is the same.
fn place(from: &Path, to: &Path, what: &str) -> Result<(), String> {
    if let Err(e) = fs::rename(from, to)
        && !to.exists()
    {
        return Err(format!(
            "Could not install {what} to {}: {e}.",
            to.display()
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_builds_the_css() {
        let root = std::env::temp_dir().join(format!("wisp-css-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("src")).unwrap();
        let css = |tool, postcss| Css { tool, postcss };
        assert_eq!(detect(&root), css(None, false));
        fs::write(root.join("postcss.config.js"), "").unwrap();
        // Nothing for PostCSS to read.
        assert_eq!(detect(&root), css(None, false));
        fs::write(root.join("src/app.css"), "a {}").unwrap();
        assert_eq!(detect(&root), css(None, true));
        fs::write(root.join("src/app.css"), "@import \"tailwindcss\";").unwrap();
        assert_eq!(detect(&root), css(Some(Tool::Tailwind), true));
        fs::write(root.join("src/app.scss"), "a { b { c: d } }").unwrap();
        assert_eq!(detect(&root), css(Some(Tool::Sass), true));
        assert_eq!(detect(&root).tool_out(), ".wisp/pre.css");
        fs::remove_file(root.join("postcss.config.js")).unwrap();
        let sass = detect(&root);
        let _ = fs::remove_dir_all(&root);
        assert_eq!(sass, css(Some(Tool::Sass), false));
        assert_eq!(sass.tool_out(), ".wisp/app.css");
        assert!(!sass.plain());
    }
}
