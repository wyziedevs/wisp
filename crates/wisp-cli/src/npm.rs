//! `wisp add` and `wisp remove`: the app's npm packages, pinned in
//! package.json's `dependencies` (no Node needed). And for `wisp build`,
//! the modules the app imports, downloaded once from esm.sh into
//! `.wisp/npm`, which the release build serves.

use crate::{net, term};
use std::fs;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use wisp_build::npm::{self, ESM};
use wisp_shared::json;

const REGISTRY: &str = "https://registry.npmjs.org";

/// Downloads at once at most.
const PARALLEL: usize = 16;

/// `wisp add pkg[@version] …`: each pinned to a version, `latest` unless
/// given (a version, or a tag like `next`), asked of the registry at once.
pub fn add(root: &Path, args: &[String]) -> Result<(), String> {
    if args.is_empty() {
        return Err("wisp add takes a package, like wisp add canvas-confetti.".into());
    }
    let wanted = args
        .iter()
        .map(|a| parse(a))
        .collect::<Result<Vec<_>, _>>()?;
    let versions: Vec<Result<String, String>> = std::thread::scope(|s| {
        let asks: Vec<_> = wanted
            .iter()
            .map(|&(name, want)| s.spawn(move || resolve(name, want.unwrap_or("latest"))))
            .collect();
        asks.into_iter()
            .map(|a| {
                a.join()
                    .unwrap_or_else(|_| Err("A registry lookup failed.".into()))
            })
            .collect()
    });
    let mut added = Vec::with_capacity(wanted.len());
    for (&(name, _), version) in wanted.iter().zip(versions) {
        added.push((name, version?));
    }
    let changes: Vec<_> = added.iter().map(|(n, v)| (*n, Some(v.as_str()))).collect();
    save(root, &changes)?;
    for (name, version) in &added {
        term::done(&format!("Added {name}@{version}"));
        println!("    {}", term::accent(&format!("import x from '{name}'")));
    }
    Ok(())
}

/// `wisp remove pkg …`.
pub fn remove(root: &Path, args: &[String]) -> Result<(), String> {
    if args.is_empty() {
        return Err("wisp remove takes a package, like wisp remove canvas-confetti.".into());
    }
    let changes = args
        .iter()
        .map(|a| parse(a).map(|(name, _)| (name, None)))
        .collect::<Result<Vec<_>, _>>()?;
    save(root, &changes)?;
    for (name, _) in changes {
        term::done(&format!("Removed {name}"));
    }
    Ok(())
}

/// `pkg`, `pkg@1.2.3`, `@scope/pkg@next`: the name, and the version asked for.
fn parse(arg: &str) -> Result<(&str, Option<&str>), String> {
    let at = arg
        .char_indices()
        .skip(1)
        .find(|&(_, c)| c == '@')
        .map(|(i, _)| i);
    let (name, want) = match at {
        Some(i) => (&arg[..i], Some(&arg[i + 1..])),
        None => (arg, None),
    };
    if !npm::valid_name(name) {
        return Err(format!(
            "{name} is not an npm package name.\nNames are lower case, like canvas-confetti or @scope/pkg."
        ));
    }
    if want.is_some_and(|w| {
        w.is_empty()
            || !w
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b".-+_".contains(&b))
    }) {
        return Err(format!(
            "{arg}: give a version (1.2.3) or a tag (latest), like wisp add {name}@1.2.3."
        ));
    }
    Ok((name, want))
}

/// The version of `name` that `want` (a version or a tag) is, by the registry.
fn resolve(name: &str, want: &str) -> Result<String, String> {
    let url = format!("{REGISTRY}/{name}/{want}");
    match net::fetch(&url) {
        Ok(body) => json::parse(&String::from_utf8_lossy(&body))
            .ok()
            .and_then(|doc| doc.str("version").map(str::to_string))
            .filter(|v| npm::exact(v))
            .ok_or_else(|| {
                format!("The npm registry's answer for {name}@{want} has no version.\nTried {url}.")
            }),
        Err(e) if e.status => Err(format!(
            "npm has no {name}@{want}.\nCheck the name at https://www.npmjs.com/package/{name}."
        )),
        Err(e) => Err(format!(
            "Could not ask the npm registry for {name}: {}.\nTried {url}. Check the network.",
            e.text
        )),
    }
}

/// package.json with each package pinned to its version, or taken out
/// for `None`, written once.
fn save(root: &Path, changes: &[(&str, Option<&str>)]) -> Result<(), String> {
    let path = root.join("package.json");
    let mut text = wisp_build::read_source(&path).ok();
    for &(name, version) in changes {
        text = Some(npm::set_dep(text.as_deref(), name, version)?);
    }
    fs::write(&path, text.unwrap_or_default())
        .map_err(|e| format!("Could not write package.json: {e}."))
}

/// Downloads the npm modules the app imports (`check` lists them), and
/// the modules those import, into `.wisp/npm`, those not there from an
/// earlier build: a pinned version's files never change.
pub fn vendor(root: &Path, imports: &[String]) -> Result<(), String> {
    if imports.is_empty() {
        return Ok(());
    }
    let dir = root.join(".wisp").join("npm");
    let mut first = true;
    npm::walk(&dir, imports, |paths| {
        if std::mem::take(&mut first) {
            term::step("Downloading npm packages from esm.sh into .wisp/npm");
        }
        download(&dir, paths)
    })
    .map(drop)
}

/// Each module at `paths`, `PARALLEL` at a time.
fn download(dir: &Path, paths: &[String]) -> Result<(), String> {
    let next = AtomicUsize::new(0);
    let failed = AtomicBool::new(false);
    std::thread::scope(|s| {
        let workers: Vec<_> = (0..paths.len().min(PARALLEL))
            .map(|_| {
                s.spawn(|| {
                    while !failed.load(Ordering::Relaxed) {
                        let Some(path) = paths.get(next.fetch_add(1, Ordering::Relaxed)) else {
                            break;
                        };
                        if let Err(e) = fetch(dir, path) {
                            failed.store(true, Ordering::Relaxed);
                            return Err(e);
                        }
                    }
                    Ok(())
                })
            })
            .collect();
        workers.into_iter().try_for_each(|w| {
            w.join()
                .unwrap_or_else(|_| Err("A download failed.".into()))
        })
    })
}

/// One module, its imports pointed at the app's copies.
fn fetch(dir: &Path, path: &str) -> Result<(), String> {
    let url = format!("{ESM}{path}");
    let dest = dir.join(npm::file(path)?);
    if let Some(parent) = dest.parent() {
        crate::make_dir(parent)?;
    }
    let text = |url: &str, body: Vec<u8>| {
        String::from_utf8(body).map_err(|_| format!("{url} is not a JavaScript module."))
    };
    net::fetch_to(&url, &dest, |partial| {
        let src = text(&url, fs::read(partial).map_err(|e| format!("{e}"))?)?;
        let src = npm::rewrite(path, &src, |stub| {
            let url = format!("{ESM}{stub}");
            let body =
                net::fetch(&url).map_err(|e| format!("Could not download {url}: {}.", e.text))?;
            text(&url, body)
        })?;
        fs::write(partial, src).map_err(|e| format!("Could not write {}: {e}.", dest.display()))
    })
    .map_err(|e| {
        format!("{e}\nCheck the network; each package is downloaded once, then kept in .wisp/npm.")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn package_args() {
        assert_eq!(parse("canvas-confetti"), Ok(("canvas-confetti", None)));
        assert_eq!(parse("x@1.2.3"), Ok(("x", Some("1.2.3"))));
        assert_eq!(parse("@s/p"), Ok(("@s/p", None)));
        assert_eq!(parse("@s/p@next"), Ok(("@s/p", Some("next"))));
        for bad in ["X", "x@", "x@^1", "x@1 || 2", "@s", "../x"] {
            assert!(parse(bad).is_err(), "{bad}");
        }
    }
}
