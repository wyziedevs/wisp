//! `wisp add` and `wisp remove`: the app's npm packages, pinned in
//! package.json's `dependencies` (no Node needed). And for `wisp build`,
//! the packages the app imports, downloaded once from esm.sh into
//! `.wisp/npm`, which the release build serves.

use crate::{net, term};
use std::collections::HashSet;
use std::fs;
use std::path::Path;
use wisp_build::npm;

const REGISTRY: &str = "https://registry.npmjs.org";

/// `wisp add pkg[@version] …`: each pinned to a version, `latest` unless
/// given (a version, or a tag like `next`).
pub fn add(root: &Path, args: &[String]) -> Result<(), String> {
    if args.is_empty() {
        return Err("wisp add takes a package, like wisp add canvas-confetti.".into());
    }
    for arg in args {
        let (name, want) = parse(arg)?;
        let version = resolve(name, want.unwrap_or("latest"))?;
        save(root, name, Some(&version))?;
        term::done(&format!("Added {name}@{version}"));
        println!("    import x from '{name}'");
    }
    Ok(())
}

/// `wisp remove pkg …`.
pub fn remove(root: &Path, args: &[String]) -> Result<(), String> {
    if args.is_empty() {
        return Err("wisp remove takes a package, like wisp remove canvas-confetti.".into());
    }
    for arg in args {
        let (name, _) = parse(arg)?;
        save(root, name, None)?;
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
    match net::fetch(&url, None) {
        Ok(body) => {
            npm::string_member(&String::from_utf8_lossy(&body), "version").ok_or_else(|| {
                format!("The npm registry's answer for {name}@{want} has no version.\nTried {url}.")
            })
        }
        Err(net::Fail::Status) => Err(format!(
            "npm has no {name}@{want}.\nCheck the name at https://www.npmjs.com/package/{name}."
        )),
        Err(e) => Err(format!(
            "Could not ask the npm registry for {name}: {}.\nTried {url}. Check the network.",
            e.text()
        )),
    }
}

fn save(root: &Path, name: &str, version: Option<&str>) -> Result<(), String> {
    let path = root.join("package.json");
    let old = fs::read_to_string(&path).ok();
    let new = npm::set_dep(old.as_deref(), name, version)?;
    fs::write(&path, new).map_err(|e| format!("Could not write package.json: {e}."))
}

/// Downloads each package module the app imports, and the modules those
/// import, into `.wisp/npm`, unless it is there from an earlier build: a
/// pinned version's files never change.
pub fn vendor(root: &Path) -> Result<(), String> {
    let mut stack = wisp_build::npm_used(root)?;
    let dir = root.join(".wisp").join("npm");
    let mut seen = HashSet::new();
    let mut fetched = 0;
    while let Some(path) = stack.pop() {
        if !seen.insert(path.clone()) {
            continue;
        }
        let file = dir.join(npm::local(&path)?);
        let src = match fs::read_to_string(&file) {
            Ok(src) => src,
            Err(_) => {
                if fetched == 0 {
                    term::step("Downloading npm packages from esm.sh into .wisp/npm");
                }
                fetched += 1;
                fetch(&path, &file)?
            }
        };
        stack.extend(npm::imports(&src));
    }
    Ok(())
}

/// One module, written whole or not at all.
fn fetch(path: &str, file: &Path) -> Result<String, String> {
    let url = format!("{}{path}", npm::ESM);
    let body = net::fetch(&url, None).map_err(|e| {
        format!(
            "Could not download {url} for the release build: {}.\nCheck the network; each package is downloaded once, then kept in .wisp/npm.",
            e.text()
        )
    })?;
    let src = String::from_utf8(body).map_err(|_| format!("{url} is not a JavaScript module."))?;
    let io = |e: std::io::Error| format!("Could not write {}: {e}.", file.display());
    if let Some(parent) = file.parent() {
        fs::create_dir_all(parent).map_err(io)?;
    }
    let mut partial = file.as_os_str().to_owned();
    partial.push(format!(".{}.download", std::process::id()));
    fs::write(&partial, &src).map_err(io)?;
    fs::rename(&partial, file).map_err(io)?;
    Ok(src)
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
