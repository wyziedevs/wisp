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
    // A crate's name, so never a path: `..` or `/` would reach outside.
    if let Some(n) = names.iter().find(|n| {
        n.is_empty()
            || !n
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    }) {
        return Err(format!("plugin `{n}`: not a crate name"));
    }
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

/// `base = "/app"` of `[package.metadata.wisp]` in the app's Cargo.toml:
/// the path it is served under (see `protocol::BASE`).
pub fn base(root: &Path) -> Option<String> {
    base_of(&fs::read_to_string(root.join("Cargo.toml")).ok()?)
}

fn base_of(toml: &str) -> Option<String> {
    let mut on = false;
    for l in toml.lines() {
        let l = l.trim();
        if l.starts_with('[') {
            on = l == "[package.metadata.wisp]";
        } else if on && let Some(v) = l.strip_prefix("base").map(str::trim_start) {
            let v = v.strip_prefix('=')?.trim();
            return Some(v.trim_matches(['"', '\'']).to_string());
        }
    }
    None
}

/// The crate names of `use = [...]` in `[package.metadata.wisp]`.
fn used(toml: &str) -> Vec<String> {
    let mut on = false;
    let mut text = String::new();
    for l in toml.lines() {
        let l = l.trim();
        if l.starts_with('[') {
            on = l == "[package.metadata.wisp]";
        } else if on && !l.starts_with("base") {
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
            && let Some(p) = path_of(l)
        {
            return Ok(root.join(p));
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

/// The `path = "..."` (or `'...'`) of a TOML line.
fn path_of(line: &str) -> Option<String> {
    let mut rest = line;
    while let Some(k) = rest.find("path") {
        let word = rest[..k]
            .chars()
            .next_back()
            .is_some_and(|c| c.is_alphanumeric() || c == '_' || c == '-');
        rest = &rest[k + 4..];
        let Some(v) = rest.trim_start().strip_prefix('=').filter(|_| !word) else {
            continue;
        };
        let v = v.trim_start();
        let q = v.chars().next().filter(|c| *c == '"' || *c == '\'')?;
        let v = &v[1..];
        let v = &v[..v.find(q)?];
        return Some(if q == '"' {
            v.replace("\\\\", "\\")
        } else {
            v.into()
        });
    }
    None
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
    // A folder of the app's own is never replaced.
    let ours = fs::read_to_string(to.join(".gitignore")).is_ok_and(|s| s == MARK);
    if !have.is_empty() && !ours {
        return Err(format!(
            "{} is the app's own, and a plugin's files go there: rename it",
            to.display()
        ));
    }
    let same = want.len() == have.len()
        && want
            .iter()
            .all(|(r, p)| fs::read(p).is_ok_and(|a| fs::read(to.join(r)).is_ok_and(|b| a == b)))
        && (want.is_empty() || ours);
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
    fn reads_the_base_path() {
        let t =
            "[package]\nname = \"a\"\n[package.metadata.wisp]\nbase = \"/user\"\nuse = [\"kit\"]\n";
        assert_eq!(base_of(t).as_deref(), Some("/user"));
        assert_eq!(used(t), ["kit"]);
        assert_eq!(base_of("[package]\nbase = \"/x\"\n"), None);
    }

    #[test]
    fn reads_use_and_paths() {
        let t = "[package]\nname = \"a\"\n[package.metadata.wisp]\nuse = [\n \"kit-x\",\n \"y\"\n]\n[dependencies]\nkit-x = { path = \"../kit\", version = \"1\" }\n";
        assert_eq!(used(t), ["kit-x", "y"]);
        assert_eq!(
            find(Path::new("/r"), t, "kit-x").unwrap(),
            Path::new("/r/../kit")
        );
        assert!(find(Path::new("/r"), t, "y").is_err());
        // Another key holding "path", single quotes, a Windows path.
        assert_eq!(
            path_of(r#"k = { git = "https://x/path", path = 'a/b' }"#),
            Some("a/b".into())
        );
        assert_eq!(path_of(r#"k = { xpath = "no" }"#), None);
        assert_eq!(path_of(r#"k.path = "..\kit""#), Some(r"..\kit".into()));
        let d = std::env::temp_dir().join(format!("wisp-plugin-name-{}", std::process::id()));
        fs::create_dir_all(&d).unwrap();
        fs::write(
            d.join("Cargo.toml"),
            "[package.metadata.wisp]\nuse = [\"../x\"]\n",
        )
        .unwrap();
        assert!(sync(&d).unwrap_err().contains("not a crate name"));
        let _ = fs::remove_dir_all(&d);
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
        // A folder of the app's own where a plugin's go: refused, untouched.
        let own = app.join("src/components/kit");
        fs::remove_file(own.join(".gitignore")).unwrap();
        assert!(sync(&app).unwrap_err().contains("the app's own"));
        assert!(own.join("Badge.wisp").is_file());
        fs::write(own.join(".gitignore"), MARK).unwrap();
        // Dropped from `use`: gone, and the app's own stays.
        fs::write(app.join("Cargo.toml"), "[package]\n").unwrap();
        sync(&app).unwrap();
        assert!(!app.join("src/routes/(kit)").exists());
        assert!(!app.join("src/components/kit").exists());
        assert!(app.join("src/components/Mine.wisp").is_file());
        let _ = fs::remove_dir_all(&d);
    }
}
