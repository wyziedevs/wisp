//! The CSS step. If `src/app.css` imports Tailwind, the Tailwind standalone
//! CLI builds it into `.wisp/app.css`; otherwise `src/app.css` is served as
//! written and there is nothing to run.

use crate::{sha256, term};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

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

pub enum Css {
    /// No `src/app.css`, or a plain one served as is.
    Plain,
    Tailwind,
}

pub fn detect(root: &Path) -> Css {
    let src = fs::read_to_string(root.join("src").join("app.css")).unwrap_or_default();
    if src.contains("@import \"tailwindcss\"") || src.contains("@import 'tailwindcss'") {
        Css::Tailwind
    } else {
        Css::Plain
    }
}

/// Kills the watcher when dropped. Its stdin is a pipe we hold open: when the
/// CLI dies for any reason, the pipe closes and Tailwind exits by itself.
pub struct Watcher(Child);

impl Drop for Watcher {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

pub fn watch(root: &Path) -> Result<Option<Watcher>, String> {
    let Css::Tailwind = detect(root) else {
        // A stale Tailwind build would shadow the plain src/app.css.
        let _ = fs::remove_file(root.join(".wisp").join("app.css"));
        return Ok(None);
    };
    let bin = tailwind()?;
    let mut child = Command::new(bin)
        .args(["-i", "src/app.css", "-o", ".wisp/app.css", "--watch"])
        .current_dir(root)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("Could not start Tailwind: {e}."))?;
    // Tailwind reports every build on stderr; `wisp dev` already says when
    // the styles change, so only its errors get through.
    let stderr = child.stderr.take().expect("stderr is piped");
    std::thread::spawn(move || {
        term::each_line(stderr, |line| {
            if let Some(e) = line.strip_prefix("Error: ") {
                term::failed(&format!("Tailwind could not build the CSS.\n{e}"));
            } else if !(line.is_empty()
                || line.starts_with("≈ tailwindcss")
                || line.starts_with("Done in "))
            {
                eprintln!("{line}");
            }
        });
    });
    Ok(Some(Watcher(child)))
}

/// One minified build, for `wisp build`.
pub fn build(root: &Path) -> Result<(), String> {
    let built = root.join(".wisp").join("app.css");
    let Css::Tailwind = detect(root) else {
        // A stale Tailwind build would shadow the plain src/app.css.
        let _ = fs::remove_file(built);
        return Ok(());
    };
    let status = Command::new(tailwind()?)
        .args(["-i", "src/app.css", "-o", ".wisp/app.css", "--minify"])
        .current_dir(root)
        .status()
        .map_err(|e| format!("Could not run Tailwind: {e}."))?;
    if status.success() {
        Ok(())
    } else {
        Err("Tailwind could not build the CSS.\nIts errors are above.".into())
    }
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
    let (_, _, asset, sha) = TAILWIND_ASSETS
        .iter()
        .find(|(os, arch, _, _)| *os == std::env::consts::OS && *arch == std::env::consts::ARCH)
        .ok_or_else(|| {
            format!(
                "Tailwind has no build for {}-{}.\nSet WISP_TAILWIND to a tailwindcss binary.",
                std::env::consts::OS,
                std::env::consts::ARCH
            )
        })?;
    let home = std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .ok_or("Could not find the home folder.\nSet HOME or USERPROFILE.")?;
    let dir = PathBuf::from(home).join(".wisp").join("bin");
    let bin = dir.join(format!(
        "tailwindcss-{TAILWIND_VERSION}{}",
        std::env::consts::EXE_SUFFIX
    ));
    if bin.exists() {
        return Ok(bin);
    }

    fs::create_dir_all(&dir).map_err(|e| format!("Could not create {}: {e}.", dir.display()))?;
    let url = format!(
        "https://github.com/tailwindlabs/tailwindcss/releases/download/{TAILWIND_VERSION}/{asset}"
    );
    let partial = bin.with_extension("download");
    crate::term::step(&format!(
        "Downloading Tailwind {TAILWIND_VERSION} from {url}. This happens once."
    ));
    // curl ships with Windows 10+, macOS and every Linux distribution.
    let status = Command::new("curl")
        .args(["-fL", "--progress-bar", "-o"])
        .arg(&partial)
        .arg(&url)
        .status()
        .map_err(|e| format!("Could not run curl: {e}."))?;
    if !status.success() {
        let _ = fs::remove_file(&partial);
        return Err(format!("Could not download Tailwind.\nTried {url}."));
    }
    let bytes = fs::read(&partial).map_err(|e| e.to_string())?;
    let got = sha256::hex_digest(&bytes);
    if got != *sha {
        let _ = fs::remove_file(&partial);
        return Err(format!(
            "The Tailwind download is not the expected file, so it was not used.\nExpected SHA-256 {sha}, got {got}."
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&partial, fs::Permissions::from_mode(0o755))
            .map_err(|e| e.to_string())?;
    }
    fs::rename(&partial, &bin).map_err(|e| e.to_string())?;
    Ok(bin)
}
