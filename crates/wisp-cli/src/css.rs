//! The CSS step, for an app that opts in. `src/app.scss` is built by Sass,
//! or a `src/app.css` that imports Tailwind by Tailwind, each a standalone
//! binary (no Node), into `.wisp/app.css`; with a `postcss.config.*`, the
//! app's PostCSS (it needs Node) runs on the result, or on a plain
//! `src/app.css`. Otherwise `src/app.css` is served as written and there
//! is nothing to run.

use crate::{net, term};
use std::ffi::OsString;
use std::fs;
use std::io::Read;
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

/// The files that turn PostCSS on, at the app's top.
pub const POSTCSS_CONFIGS: [&str; 6] = [
    "postcss.config.js",
    "postcss.config.cjs",
    "postcss.config.mjs",
    "postcss.config.ts",
    "postcss.config.json",
    ".postcssrc.json",
];

/// The app's own PostCSS, which node runs itself: no npx, and no shell
/// between `wisp dev` and node to outlive it.
const POSTCSS_CLI: &str = "node_modules/postcss-cli/index.js";

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Tool {
    Tailwind,
    Sass,
    /// Runs after the others (`Css::tool` is never it), or on a plain
    /// `src/app.css`.
    PostCss,
}

impl Tool {
    fn name(self) -> &'static str {
        match self {
            Tool::Tailwind => "Tailwind",
            Tool::Sass => "Sass",
            Tool::PostCss => "PostCSS",
        }
    }

    /// A line the tool's watcher prints on every build, which `wisp dev`
    /// leaves out: it says when the styles change itself.
    fn chatter(self, line: &str) -> bool {
        line.is_empty()
            || match self {
                Tool::Tailwind => line.starts_with("≈ tailwindcss") || line.starts_with("Done in "),
                Tool::Sass => false,
                Tool::PostCss => {
                    line.starts_with("Processing ")
                        || line.starts_with("Finished ")
                        || line.starts_with("Waiting for file changes")
                }
            }
    }

    fn cannot_run(self, e: std::io::Error) -> String {
        match self {
            Tool::PostCss => format!(
                "Could not run node for PostCSS: {e}.\nPostCSS needs Node: install it, then npm install -D postcss postcss-cli. Or remove postcss.config.*."
            ),
            _ => format!("Could not run {}: {e}.", self.name()),
        }
    }
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

    /// Whether a CSS tool reads `rel` (relative to the app, `/`-separated):
    /// its watcher sees a change and writes `.wisp/app.css`.
    pub fn owns(&self, rel: &str) -> bool {
        match self.tool {
            Some(Tool::Sass) => {
                rel.starts_with("src/") && (rel.ends_with(".scss") || rel.ends_with(".sass"))
            }
            Some(_) => rel == "src/app.css",
            None => self.postcss && rel == "src/app.css",
        }
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

/// Whether `rel` (relative to the app) is a file `detect` reads.
pub fn decides(rel: &str) -> bool {
    matches!(rel, "src/app.css" | "src/app.scss") || POSTCSS_CONFIGS.contains(&rel)
}

/// A watcher, run by `wisp __child` (see `child`): dropped, it closes the
/// pipe that is its stdin, and its tool goes.
pub struct Watcher(Child);

impl Drop for Watcher {
    fn drop(&mut self) {
        drop(self.0.stdin.take());
        let _ = self.0.wait();
    }
}

/// `wisp __child <exe> <args…>`: runs the tool until this process's stdin
/// closes, then kills it. `wisp dev` holds that stdin, and the system
/// closes it however `wisp dev` ends, so no tool outlives it. The tool's
/// own stdin is a pipe held open (Tailwind's watcher stops at its end).
pub fn child(args: &[String]) -> Result<(), String> {
    let (exe, args) = args.split_first().ok_or("wisp __child takes a program.")?;
    let mut tool = Command::new(exe)
        .args(args)
        .stdin(Stdio::piped())
        .spawn()
        .map_err(|e| format!("Could not start {exe}: {e}."))?;
    let mut buf = [0u8; 64];
    while matches!(std::io::stdin().read(&mut buf), Ok(n) if n > 0) {}
    let _ = tool.kill();
    let _ = tool.wait();
    Ok(())
}

/// The watchers that rebuild `.wisp/app.css` on every change, for `wisp
/// dev`; each makes the first build itself.
pub fn watch(root: &Path, css: &Css) -> Result<Vec<Watcher>, String> {
    if !prepare(root, css)? {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    if let Some(tool) = css.tool {
        out.push(watcher(root, tool, css)?);
    }
    if css.postcss {
        // PostCSS watches the tool's output, which must be there; it sees
        // the tool's first build when that is written.
        let input = root.join(css.postcss_in());
        if !input.exists() {
            fs::write(&input, "")
                .map_err(|e| format!("Could not write {}: {e}.", input.display()))?;
        }
        out.push(watcher(root, Tool::PostCss, css)?);
    }
    Ok(out)
}

/// One minified build, for `wisp build`.
pub fn build(root: &Path) -> Result<(), String> {
    let css = detect(root);
    if !prepare(root, &css)? {
        return Ok(());
    }
    if let Some(tool) = css.tool {
        let mut cmd = command(root, tool, &css)?;
        cmd.arg(match tool {
            Tool::Sass => "--style=compressed",
            _ => "--minify",
        });
        run(&mut cmd, tool)?;
    }
    if css.postcss {
        run(&mut command(root, Tool::PostCss, &css)?, Tool::PostCss)?;
    }
    Ok(())
}

/// Whether there is a CSS step to run, after making `.wisp/` for it. Plain
/// CSS has none, and a stale build would shadow `src/app.css`: it goes.
fn prepare(root: &Path, css: &Css) -> Result<bool, String> {
    let dir = root.join(".wisp");
    if css.plain() {
        let _ = fs::remove_file(dir.join("app.css"));
        return Ok(false);
    }
    crate::make_dir(&dir)?;
    Ok(true)
}

/// Runs a build to its end; the tool's errors are on the terminal.
fn run(cmd: &mut Command, tool: Tool) -> Result<(), String> {
    let status = cmd.status().map_err(|e| tool.cannot_run(e))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!(
            "{} could not build the CSS.\nIts errors are above.",
            tool.name()
        ))
    }
}

/// The tool's `--watch`, run by `wisp __child`. It reports every build on
/// stderr; only its errors (and what is not `chatter`) get through.
fn watcher(root: &Path, tool: Tool, css: &Css) -> Result<Watcher, String> {
    let mut cmd = command(root, tool, css)?;
    cmd.arg("--watch");
    let me = std::env::current_exe().map_err(|e| format!("Could not find wisp itself: {e}."))?;
    let mut child = Command::new(me)
        .arg("__child")
        .arg(cmd.get_program())
        .args(cmd.get_args())
        .current_dir(root)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| tool.cannot_run(e))?;
    let stderr = child.stderr.take().expect("stderr is piped");
    std::thread::spawn(move || {
        term::each_line(stderr, |line| {
            if let Some(e) = line.strip_prefix("Error: ") {
                term::failed(&format!("{} could not build the CSS.\n{e}", tool.name()));
            } else if !tool.chatter(line) {
                eprintln!("{line}");
            }
        });
    });
    Ok(Watcher(child))
}

/// The tool's command, from its input to its output, run in the app.
fn command(root: &Path, tool: Tool, css: &Css) -> Result<Command, String> {
    let mut cmd = match tool {
        Tool::Tailwind => {
            let mut cmd = Command::new(tailwind()?);
            cmd.args(["-i", "src/app.css", "-o", css.tool_out()]);
            cmd
        }
        Tool::Sass => {
            let mut cmd = sass()?;
            cmd.args(["--no-source-map", "src/app.scss", css.tool_out()]);
            cmd
        }
        Tool::PostCss => {
            if !root.join(POSTCSS_CLI).exists() {
                return Err(format!(
                    "PostCSS is not installed: there is no {POSTCSS_CLI}.\nInstall it with npm install -D postcss postcss-cli, or remove postcss.config.*."
                ));
            }
            let mut cmd = Command::new("node");
            cmd.args([POSTCSS_CLI, css.postcss_in(), "-o", ".wisp/app.css"]);
            cmd
        }
    };
    cmd.current_dir(root);
    Ok(cmd)
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
    if !bin.exists() {
        let url = format!(
            "https://github.com/tailwindlabs/tailwindcss/releases/download/{TAILWIND_VERSION}/{asset}"
        );
        download(
            &format!("Tailwind {TAILWIND_VERSION}"),
            &url,
            sha,
            &bin,
            set,
        )?;
    }
    Ok(bin)
}

/// `$WISP_SASS` (a sass binary), else the pinned Dart Sass in
/// `~/.wisp/bin`, downloaded, verified and unpacked on first use: its Dart
/// runtime, running the snapshot that is Sass.
fn sass() -> Result<Command, String> {
    if let Some(p) = std::env::var_os("WISP_SASS") {
        return Ok(Command::new(p));
    }
    let set = "set WISP_SASS to a sass binary";
    let (asset, sha) = asset(SASS_ASSETS, "Dart Sass", set)?;
    let dir = bin_dir()?.join(format!("dart-sass-{SASS_VERSION}"));
    if !dir.exists() {
        let url = format!(
            "https://github.com/sass/dart-sass/releases/download/{SASS_VERSION}/dart-sass-{SASS_VERSION}-{asset}"
        );
        // Downloaded and unpacked beside it, then moved into place whole.
        let mut tmp = OsString::from(dir.as_os_str());
        tmp.push(format!(".{}.tmp", std::process::id()));
        let tmp = PathBuf::from(tmp);
        let archive = tmp.with_extension("archive");
        let unpacked = download(&format!("Sass {SASS_VERSION}"), &url, sha, &archive, set)
            .and_then(|()| crate::make_dir(&tmp))
            .and_then(|()| untar(&archive, &tmp))
            .and_then(|()| place(&tmp.join("dart-sass"), &dir, "Sass"));
        let _ = fs::remove_file(&archive);
        let _ = fs::remove_dir_all(&tmp);
        unpacked?;
    }
    let src = dir.join("src");
    let mut cmd = Command::new(src.join(format!("dart{}", std::env::consts::EXE_SUFFIX)));
    cmd.arg(src.join("sass.snapshot"));
    Ok(cmd)
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
    crate::make_dir(&dir)?;
    Ok(dir)
}

/// Downloads `url` to `dest`, if it is the file expected, executable.
fn download(what: &str, url: &str, sha: &str, dest: &Path, set: &str) -> Result<(), String> {
    term::step(&format!(
        "Downloading {what} from {url}. This happens once."
    ));
    net::fetch_to(url, dest, |file| {
        let bytes = fs::read(file).map_err(|e| format!("{}: {e}.", file.display()))?;
        let got = hex(&sha256(&[&bytes]));
        if got != sha {
            return Err(format!(
                "The {what} download is not the expected file, so it was not used.\nExpected SHA-256 {sha}, got {got}."
            ));
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(file, fs::Permissions::from_mode(0o755))
                .map_err(|e| format!("{}: {e}.", file.display()))?;
        }
        Ok(())
    })
    .map_err(|e| format!("{e}\nCheck the network, or {set}."))
}

/// Moves an unpacked download into place. Another `wisp` may have
/// finished first; its copy is the same.
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
        assert!(detect(&root).owns("src/app.css"));
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
        // Sass reads src's .scss files; src/app.css and static's are not its.
        assert!(sass.owns("src/app.scss") && sass.owns("src/styles/_x.scss"));
        assert!(!sass.owns("src/app.css") && !sass.owns("static/x.scss"));
        assert!(!css(None, false).owns("src/app.css"));
        assert!(decides("postcss.config.js") && decides("src/app.scss") && !decides("src/x.scss"));
    }
}
