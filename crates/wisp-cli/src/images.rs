//! The images step of a release build: each JPEG, PNG or WebP a template
//! shows, at up to three widths as WebP, into `.wisp/img` (named by content
//! hash, so a second build encodes nothing), by a pinned cwebp (libwebp's
//! own encoder, no Node). Without cwebp the build warns, and the pages
//! serve the original.

use crate::{css, term};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use wisp_build::image;

pub const CWEBP_VERSION: &str = "1.6.0";

/// libwebp's release archives and their SHA-256.
const CWEBP_ASSETS: &[(&str, &str, &str, &str)] = &[
    (
        "windows",
        "x86_64",
        "windows-x64.zip",
        "48886f506b21f62e4661f0f4cbfca19800897c385128e8902542d29a950c93f1",
    ),
    (
        "linux",
        "x86_64",
        "linux-x86-64.tar.gz",
        "1c5ffab71efecefa0e3c23516c3a3a1dccb45cc310ae1095c6f14ae268e38067",
    ),
    (
        "linux",
        "aarch64",
        "linux-aarch64.tar.gz",
        "69f5eebe203e0f3942fe37986209a1725741be19c152950a4283b376c95ec798",
    ),
    (
        "macos",
        "aarch64",
        "mac-arm64.tar.gz",
        "bc6bf84cc70f3f8574fba797d1e4a7dea4feebe9fa4be919f202413ea2b3b8f2",
    ),
    (
        "macos",
        "x86_64",
        "mac-x86-64.tar.gz",
        "f112dd83b420ab2a4b27d46610d9827ddf4200216023281de378647ecca31c2a",
    ),
];

/// A width to write: the image, the width, whether that is smaller than
/// it, and the file.
struct Job {
    from: PathBuf,
    width: u32,
    resize: bool,
    to: PathBuf,
}

/// Writes the WebP widths that are not there yet. Never fails the build:
/// what cannot be encoded is served as it is, with a warning.
pub fn build(root: &Path) {
    webp(root);
    #[cfg(feature = "avif")]
    avif(root);
}

/// The `avif` feature: the same widths as AVIF, in this process (a pure
/// Rust encoder: slow, so named by content hash and kept between builds).
#[cfg(feature = "avif")]
fn avif(root: &Path) {
    let dir = root.join(image::DIR);
    let mut jobs = Vec::new();
    for found in image::sources(root).into_iter().filter(|f| f.webp) {
        let Ok(bytes) = std::fs::read(&found.file) else {
            continue;
        };
        let Some(size) = image::size(&bytes).filter(|s| !s.turned) else {
            continue;
        };
        let hash = image::hash(&bytes);
        for width in image::widths(size.width) {
            let to = dir.join(image::avif_name(&hash, width));
            if !to.is_file() {
                jobs.push((found.file.clone(), width, to));
            }
        }
    }
    if jobs.is_empty() {
        return;
    }
    if let Err(e) = crate::make_dir(&dir) {
        return term::warn(&format!("The images get no AVIF widths.\n    {e}"));
    }
    term::step(&format!(
        "Encoding {} AVIF images into .wisp/img",
        jobs.len()
    ));
    let cores = std::thread::available_parallelism().map_or(1, |n| n.get());
    let per = jobs.len().div_ceil(cores);
    let fails: Vec<String> = std::thread::scope(|s| {
        let parts: Vec<_> = (jobs.chunks(per))
            .map(|part| {
                s.spawn(move || {
                    let mut fails = Vec::new();
                    for (from, width, to) in part {
                        let done = image::encode_avif(from, *width).and_then(|b| {
                            let mut tmp = to.clone().into_os_string();
                            tmp.push(format!(".{}.tmp", std::process::id()));
                            let tmp = PathBuf::from(tmp);
                            let r = std::fs::write(&tmp, b)
                                .and_then(|()| std::fs::rename(&tmp, to))
                                .map_err(|e| format!("{}: {e}.", to.display()));
                            let _ = std::fs::remove_file(&tmp);
                            r
                        });
                        fails.extend(done.err());
                    }
                    fails
                })
            })
            .collect();
        (parts.into_iter())
            .flat_map(|p| {
                p.join()
                    .unwrap_or_else(|_| vec!["an AVIF thread stopped".into()])
            })
            .collect()
    });
    if let Some(first) = fails.first() {
        term::warn(&format!(
            "{} of {} AVIF images could not be written; those images get no AVIF.\n    {first}",
            fails.len(),
            jobs.len()
        ));
    }
}

fn webp(root: &Path) {
    let dir = root.join(image::DIR);
    let mut jobs = Vec::new();
    for found in image::sources(root).into_iter().filter(|f| f.webp) {
        let Ok(bytes) = std::fs::read(&found.file) else {
            continue;
        };
        let Some(size) = image::size(&bytes).filter(|s| !s.turned) else {
            continue;
        };
        let hash = image::hash(&bytes);
        for width in image::widths(size.width) {
            let to = dir.join(image::webp_name(&hash, width));
            if !to.is_file() {
                jobs.push(Job {
                    from: found.file.clone(),
                    width,
                    resize: width < size.width,
                    to,
                });
            }
        }
    }
    // The app's icon, at the widths its manifest lists.
    if let Some((size, widths)) = image::icon(root) {
        for (width, name) in widths {
            let to = dir.join(name);
            if !to.is_file() {
                jobs.push(Job {
                    from: root.join(image::ICON),
                    width,
                    resize: width < size.width,
                    to,
                });
            }
        }
    }
    if jobs.is_empty() {
        return;
    }
    let tool = cwebp().and_then(|tool| crate::make_dir(&dir).map(|()| tool));
    let tool = match tool {
        Ok(t) => t,
        Err(e) => {
            return term::warn(&format!(
                "The images are served as they are, without WebP widths.\n    {e}"
            ));
        }
    };
    term::step(&format!(
        "Encoding {} WebP images into .wisp/img",
        jobs.len()
    ));
    let cores = std::thread::available_parallelism().map_or(1, |n| n.get());
    let per = jobs.len().div_ceil(cores);
    let fails: Vec<String> = std::thread::scope(|s| {
        let parts: Vec<_> = (jobs.chunks(per))
            .map(|part| {
                let tool = &tool;
                s.spawn(move || part.iter().filter_map(|j| encode(tool, j).err()).collect())
            })
            .collect();
        (parts.into_iter())
            .flat_map(|p| {
                p.join()
                    .unwrap_or_else(|_| vec!["cwebp's thread stopped".into()])
            })
            .collect()
    });
    if let Some(first) = fails.first() {
        term::warn(&format!(
            "{} of {} WebP images could not be written; those images are served as they are.\n    {first}",
            fails.len(),
            jobs.len()
        ));
    }
}

/// One width, written beside its file and then moved there: never half a
/// file under its name.
fn encode(tool: &Path, job: &Job) -> Result<(), String> {
    let mut tmp = job.to.clone().into_os_string();
    tmp.push(format!(".{}.tmp", std::process::id()));
    let tmp = PathBuf::from(tmp);
    let mut cmd = Command::new(tool);
    cmd.args(["-quiet", "-q", "80"]);
    if job.resize {
        cmd.args(["-resize", &job.width.to_string(), "0"]);
    }
    let out = cmd
        .arg(&job.from)
        .arg("-o")
        .arg(&tmp)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .map_err(|e| format!("Could not run {}: {e}.", tool.display()));
    let done = out.and_then(|out| {
        if !out.status.success() {
            let why = String::from_utf8_lossy(&out.stderr);
            return Err(format!(
                "cwebp could not encode {}: {}",
                job.from.display(),
                why.trim()
            ));
        }
        std::fs::rename(&tmp, &job.to).map_err(|e| format!("{}: {e}.", job.to.display()))
    });
    let _ = std::fs::remove_file(&tmp);
    done
}

/// `$WISP_CWEBP`, else the pinned cwebp in `~/.wisp/bin`, downloaded,
/// verified and taken out of libwebp's archive on first use.
fn cwebp() -> Result<PathBuf, String> {
    if let Some(p) = std::env::var_os("WISP_CWEBP") {
        return Ok(PathBuf::from(p));
    }
    let set = "set WISP_CWEBP to a cwebp binary";
    let (asset, sha) = css::asset(CWEBP_ASSETS, "cwebp", set)?;
    let exe = format!("cwebp{}", std::env::consts::EXE_SUFFIX);
    let bin = css::bin_dir()?.join(format!(
        "cwebp-{CWEBP_VERSION}{}",
        std::env::consts::EXE_SUFFIX
    ));
    if bin.exists() {
        return Ok(bin);
    }
    let name = format!("libwebp-{CWEBP_VERSION}-{asset}");
    let url =
        format!("https://storage.googleapis.com/downloads.webmproject.org/releases/webp/{name}");
    let mut tmp = bin.clone().into_os_string();
    tmp.push(format!(".{}.tmp", std::process::id()));
    let tmp = PathBuf::from(tmp);
    let archive = tmp.with_extension("archive");
    let folder = name.trim_end_matches(".zip").trim_end_matches(".tar.gz");
    let unpacked = css::download(&format!("cwebp {CWEBP_VERSION}"), &url, sha, &archive, set)
        .and_then(|()| crate::make_dir(&tmp))
        .and_then(|()| css::untar(&archive, &tmp, set))
        .and_then(|()| css::place(&tmp.join(folder).join("bin").join(exe), &bin, "cwebp"));
    let _ = std::fs::remove_file(&archive);
    let _ = std::fs::remove_dir_all(&tmp);
    unpacked.map(|()| bin)
}
