//! Plugin crates: `[package.metadata.wisp] use = ["admin-kit"]` in the
//! app's Cargo.toml merges that dependency's `wisp/routes` and
//! `wisp/components` into the app at build time.
//!
//! The files are copied (and kept in step, stale ones removed) to
//! `src/routes/(admin_kit)/`, a route group that adds nothing to a URL, and
//! `src/components/admin_kit/`, each with a `.gitignore`. The app's own
//! component of the same name wins; a route of the app's own that a plugin
//! also has is the usual duplicate-route error. Nothing is written when
//! nothing changed, so a build does not rebuild itself.

use std::fs;
use std::path::{Path, PathBuf};

/// Marks a folder as this module's, so one for a plugin since dropped goes.
const MARK: &str = "*\n# made by wisp from a plugin crate\n";

/// The plugin crates' `wisp` folders, after copying them in.
pub(crate) fn sync(root: &Path) -> Result<Vec<PathBuf>, String> {
    let toml = fs::read_to_string(root.join("Cargo.toml")).unwrap_or_default();
    let names = used(&toml);
    let ids: Vec<String> = names.iter().map(|n| n.replace('-', "_")).collect();
    for sub in ["routes", "components"] {
        for e in fs::read_dir(root.join("src").join(sub))
            .into_iter()
            .flatten()
            .flatten()
        {
            let tag = e
                .file_name()
                .to_string_lossy()
                .trim_matches(['(', ')'])
                .to_string();
            let mine = fs::read_to_string(e.path().join(".gitignore")).is_ok_and(|s| s == MARK);
            if mine && !ids.contains(&tag) {
                let _ = fs::remove_dir_all(e.path());
            }
        }
    }
    let mut dirs = Vec::new();
    for (name, id) in names.iter().zip(&ids) {
        let from = find(root, &toml, name)?.join("wisp");
        if !from.is_dir() {
            return Err(format!(
                "plugin `{name}`: no `wisp` folder in the crate (it has `wisp/routes` and `wisp/components`)"
            ));
        }
        let routes = root.join("src/routes").join(format!("({id})"));
        copy(&from.join("routes"), &routes, None)?;
        let own = root.join("src/components");
        copy(&from.join("components"), &own.join(id), Some(&own.join(id)))?;
        dirs.push(from);
    }
    Ok(dirs)
}

/// The crate names of `use = [...]` in `[package.metadata.wisp]`.
fn used(toml: &str) -> Vec<String> {
    let mut on = false;
    let mut text = String::new();
    for l in toml.lines() {
        let l = l.trim();
        if l.starts_with('[') {
            on = l == "[package.metadata.wisp]";
        } else if on {
            text += l;
            text += "\n";
        }
    }
    let Some(i) = text.find("use") else {
        return Vec::new();
    };
    let rest = &text[i..];
    let Some(a) = rest.find('[') else {
        return Vec::new();
    };
    let list = &rest[a + 1..];
    list[..list.find(']').unwrap_or(list.len())]
        .split('"')
        .skip(1)
        .step_by(2)
        .map(str::to_string)
        .collect()
}

/// Where dependency `name` is: its `path`, else its place in the registry
/// by the version Cargo.lock has.
fn find(root: &Path, toml: &str, name: &str) -> Result<PathBuf, String> {
    let mut sect = "";
    for l in toml.lines() {
        let l = l.trim();
        if l.starts_with('[') {
            sect = l;
        }
        let table = sect.ends_with(&format!("dependencies.{name}]"));
        let inline = sect.ends_with("dependencies]")
            && (l.strip_prefix(name)).is_some_and(|r| r.trim_start().starts_with(['=', '.']));
        if (table || inline)
            && l.contains('"')
            && let Some(k) = l.find("path")
        {
            return Ok(root.join(l[k..].split('"').nth(1).unwrap_or("")));
        }
    }
    let lock = fs::read_to_string(root.join("Cargo.lock")).unwrap_or_default();
    let mut ver = None;
    let mut lines = lock.lines();
    while let Some(l) = lines.next() {
        if l == format!("name = \"{name}\"") {
            ver = (lines.next())
                .and_then(|v| v.strip_prefix("version = \""))
                .map(|v| v.trim_end_matches('"').to_string());
        }
    }
    let home = std::env::var_os("CARGO_HOME")
        .map(PathBuf::from)
        .or_else(|| {
            let h = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"))?;
            Some(Path::new(&h).join(".cargo"))
        });
    if let (Some(ver), Some(home)) = (ver, home) {
        for e in fs::read_dir(home.join("registry/src"))
            .into_iter()
            .flatten()
            .flatten()
        {
            let p = e.path().join(format!("{name}-{ver}"));
            if p.is_dir() {
                return Ok(p);
            }
        }
    }
    Err(format!(
        "plugin `{name}`: not a dependency with a `path`, nor one in Cargo.lock and the registry (git dependencies are not supported)"
    ))
}

/// Makes `to` hold exactly the files of `from` (all of them, recursively),
/// writing only what differs. With `own`, a `.wisp` file whose name is
/// already a component of the app outside `own` is left out: the app wins.
fn copy(from: &Path, to: &Path, own: Option<&Path>) -> Result<(), String> {
    let mut want = Vec::new();
    files(from, from, &mut want, 0);
    if let Some(own) = own {
        let mut mine = Vec::new();
        files(
            own.parent().unwrap_or(own),
            own.parent().unwrap_or(own),
            &mut mine,
            0,
        );
        let taken = |n: &str| {
            mine.iter().any(|(r, p)| {
                !p.starts_with(own) && r.rsplit('/').next() == Some(n) && n.ends_with(".wisp")
            })
        };
        want.retain(|(r, _)| !taken(r.rsplit('/').next().unwrap_or(r)));
    }
    let mut have = Vec::new();
    files(to, to, &mut have, 0);
    let same = want.len() == have.len()
        && want
            .iter()
            .all(|(r, p)| fs::read(p).is_ok_and(|a| fs::read(to.join(r)).is_ok_and(|b| a == b)))
        && (want.is_empty() || to.join(".gitignore").is_file());
    if same {
        return Ok(());
    }
    let _ = fs::remove_dir_all(to);
    if want.is_empty() {
        return Ok(());
    }
    for (r, p) in &want {
        let dest = to.join(r);
        fs::create_dir_all(dest.parent().unwrap_or(to))
            .and_then(|()| fs::copy(p, &dest).map(drop))
            .map_err(|e| format!("{}: {e}", dest.display()))?;
    }
    fs::write(to.join(".gitignore"), MARK).map_err(|e| format!("{}: {e}", to.display()))
}

/// Every file under `dir`: its path from `base` with `/`, and the file.
fn files(dir: &Path, base: &Path, out: &mut Vec<(String, PathBuf)>, depth: usize) {
    for e in fs::read_dir(dir).into_iter().flatten().flatten() {
        let p = e.path();
        if p.is_dir() && depth < 32 {
            files(&p, base, out, depth + 1);
        } else if p.is_file() && e.file_name() != ".gitignore" {
            let r = (p.strip_prefix(base).unwrap_or(&p).to_string_lossy()).replace('\\', "/");
            out.push((r, p));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_use_and_paths() {
        let t = "[package]\nname = \"a\"\n[package.metadata.wisp]\nuse = [\n \"kit-x\",\n \"y\"\n]\n[dependencies]\nkit-x = { path = \"../kit\", version = \"1\" }\n";
        assert_eq!(used(t), ["kit-x", "y"]);
        assert_eq!(
            find(Path::new("/r"), t, "kit-x").unwrap(),
            Path::new("/r/../kit")
        );
        assert!(find(Path::new("/r"), t, "y").is_err());
    }

    #[test]
    fn merges_and_cleans() {
        let d = std::env::temp_dir().join(format!("wisp-plugins-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        let (app, kit) = (d.join("app"), d.join("kit"));
        for (p, f, s) in [
            (&kit, "wisp/routes/hello/+page.wisp", "<p>hi</p>"),
            (&kit, "wisp/components/Badge.wisp", "<b>1</b>"),
            (&kit, "wisp/components/Mine.wisp", "<b>2</b>"),
            (&app, "src/components/Mine.wisp", "<i>own</i>"),
        ] {
            fs::create_dir_all(p.join(f).parent().unwrap()).unwrap();
            fs::write(p.join(f), s).unwrap();
        }
        fs::write(
            app.join("Cargo.toml"),
            "[package.metadata.wisp]\nuse = [\"kit\"]\n[dependencies]\nkit.path = \"../kit\"\n",
        )
        .unwrap();
        let page = app.join("src/routes/(kit)/hello/+page.wisp");
        assert_eq!(sync(&app).unwrap().len(), 1);
        assert!(page.is_file());
        assert!(app.join("src/components/kit/Badge.wisp").is_file());
        assert!(!app.join("src/components/kit/Mine.wisp").exists());
        // Nothing changed: nothing rewritten.
        let t = fs::metadata(&page).unwrap().modified().unwrap();
        sync(&app).unwrap();
        assert_eq!(fs::metadata(&page).unwrap().modified().unwrap(), t);
        // Dropped from `use`: gone, and the app's own stays.
        fs::write(app.join("Cargo.toml"), "[package]\n").unwrap();
        sync(&app).unwrap();
        assert!(!app.join("src/routes/(kit)").exists());
        assert!(!app.join("src/components/kit").exists());
        assert!(app.join("src/components/Mine.wisp").is_file());
        let _ = fs::remove_dir_all(&d);
    }
}
