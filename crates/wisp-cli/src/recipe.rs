//! `wisp add <name>`: a recipe the app keeps in `add/<name>/`, applied to
//! the app. `add/<name>/recipe` is lines of
//!
//! ```text
//! dep sqlx = { version = "0.8", default-features = false, features = ["sqlite"] }
//! env DATABASE_URL=sqlite://app.db?mode=rwc
//! file src/db.rs
//! note Call crate::db::open().await? in init.
//! ```
//!
//! `dep` is a line for Cargo.toml's `[dependencies]`, `env` one for
//! `.env.example`, `file` copies `add/<name>/<path>` to `<path>` in the
//! app, `note` is printed after. Nothing runs, nothing is downloaded. Run
//! again, it adds nothing twice; a file that is there stays unless `--force`.

use crate::term;
use std::fs;
use std::path::{Component, Path, PathBuf};

/// What a recipe asks for.
#[derive(Default)]
struct Recipe {
    deps: Vec<String>,
    env: Vec<String>,
    files: Vec<String>,
    notes: Vec<String>,
}

/// `wisp add` with no name lists the recipes; with a name that is one, it
/// is applied. Else it is for npm.
pub fn is_recipe(root: &Path, args: &[String]) -> bool {
    match args.iter().find(|a| !a.starts_with("--")) {
        None => args.is_empty(),
        Some(name) => plain(name) && root.join("add").join(name).join("recipe").is_file(),
    }
}

pub fn run(root: &Path, args: &[String]) -> Result<(), String> {
    let force = args.iter().any(|a| a == "--force");
    let names: Vec<&str> = args
        .iter()
        .map(String::as_str)
        .filter(|a| *a != "--force")
        .collect();
    if names.is_empty() {
        return list(root);
    }
    let recipes = names
        .iter()
        .map(|n| load(root, n))
        .collect::<Result<Vec<_>, _>>()?;
    for (name, recipe) in names.iter().zip(&recipes) {
        apply(root, name, recipe, force)?;
    }
    Ok(())
}

fn list(root: &Path) -> Result<(), String> {
    let mut names: Vec<String> = fs::read_dir(root.join("add"))
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().join("recipe").is_file())
        .filter_map(|e| e.file_name().into_string().ok())
        .collect();
    names.sort();
    if names.is_empty() {
        println!("  No recipes in add/. wisp add pkg adds an npm package.");
    }
    for name in names {
        println!("  {}", term::accent(&name));
    }
    Ok(())
}

/// A name that is one folder: no separators, no `..`.
fn plain(name: &str) -> bool {
    let mut parts = Path::new(name).components();
    matches!(parts.next(), Some(Component::Normal(_))) && parts.next().is_none()
}

/// A path that stays inside the folder it is joined to.
fn inside(path: &str) -> bool {
    !path.is_empty()
        && Path::new(path)
            .components()
            .all(|c| matches!(c, Component::Normal(_)))
}

/// The recipe, with every file it names checked: so a mistake stops
/// everything before anything is written.
fn load(root: &Path, name: &str) -> Result<Recipe, String> {
    if !plain(name) {
        return Err(format!("{name} is not a recipe name."));
    }
    let dir = root.join("add").join(name);
    let text = fs::read_to_string(dir.join("recipe")).map_err(|_| {
        format!("There is no recipe {name}.\nA recipe is add/{name}/recipe; wisp add lists them.")
    })?;
    let mut r = Recipe::default();
    for (n, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (verb, rest) = line.split_once(char::is_whitespace).unwrap_or((line, ""));
        let rest = rest.trim();
        let at = format!("add/{name}/recipe line {}", n + 1);
        match verb {
            "dep" if dep_name(rest).is_some() => r.deps.push(rest.into()),
            "env" if rest.split_once('=').is_some_and(|(k, _)| !k.is_empty()) => {
                r.env.push(rest.into())
            }
            "file" if inside(rest) => {
                if !dir.join(rest).is_file() {
                    return Err(format!("{at}: add/{name}/{rest} is not a file."));
                }
                r.files.push(rest.into());
            }
            "note" if !rest.is_empty() => r.notes.push(rest.into()),
            "dep" | "env" | "file" | "note" => {
                return Err(format!(
                    "{at}: {line}\ndep takes name = version, env KEY=value, file takes a path inside the app, note some text."
                ));
            }
            _ => {
                return Err(format!("{at}: {verb} is not dep, env, file or note."));
            }
        }
    }
    Ok(r)
}

fn apply(root: &Path, name: &str, r: &Recipe, force: bool) -> Result<(), String> {
    let source = root.join("add").join(name);
    for dep in &r.deps {
        let name = dep_name(dep).unwrap_or_default();
        let path = root.join("Cargo.toml");
        let text = fs::read_to_string(&path).map_err(|e| format!("Cargo.toml: {e}."))?;
        match with_dep(&text, dep) {
            Some(new) => {
                fs::write(&path, new).map_err(|e| format!("Cargo.toml: {e}."))?;
                term::done(&format!("Added {name} to Cargo.toml"));
            }
            None => term::step(&format!("Cargo.toml has {name}")),
        }
    }
    for line in &r.env {
        let key = line.split_once('=').map_or("", |(k, _)| k);
        let path = root.join(".env.example");
        let text = fs::read_to_string(&path).unwrap_or_default();
        if text
            .lines()
            .any(|l| l.split_once('=').is_some_and(|(k, _)| k.trim() == key))
        {
            term::step(&format!(".env.example has {key}"));
            continue;
        }
        let gap = if text.is_empty() || text.ends_with('\n') {
            ""
        } else {
            "\n"
        };
        fs::write(&path, format!("{text}{gap}{line}\n"))
            .map_err(|e| format!(".env.example: {e}."))?;
        term::done(&format!("Added {key} to .env.example"));
    }
    for file in &r.files {
        let to: PathBuf = root.join(file);
        if to.exists() && !force {
            term::warn(&format!(
                "Kept the app's own {file}; --force writes over it."
            ));
            continue;
        }
        if let Some(dir) = to.parent() {
            crate::make_dir(dir)?;
        }
        fs::copy(source.join(file), &to).map_err(|e| format!("Could not write {file}: {e}."))?;
        term::done(&format!("Wrote {file}"));
    }
    for note in &r.notes {
        println!("    {note}");
    }
    Ok(())
}

/// The crate a `[dependencies]` line is for: `sqlx` of `sqlx = "0.8"`.
fn dep_name(line: &str) -> Option<&str> {
    let name = line.split(['=', '.', ' ']).next()?;
    let ok = !name.is_empty()
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
    (ok && line[name.len()..].trim_start().starts_with(['=', '.'])).then_some(name)
}

/// Cargo.toml with `dep` in `[dependencies]`; None if it has one for that
/// crate already.
fn with_dep(toml: &str, dep: &str) -> Option<String> {
    let name = dep_name(dep)?;
    let has = toml.lines().any(|l| dep_name(l.trim()) == Some(name));
    if has {
        return None;
    }
    let mut out = String::with_capacity(toml.len() + dep.len() + 20);
    let mut added = false;
    for line in toml.lines() {
        out.push_str(line);
        out.push('\n');
        if !added && line.trim() == "[dependencies]" {
            out.push_str(dep);
            out.push('\n');
            added = true;
        }
    }
    if !added {
        out.push_str(&format!("\n[dependencies]\n{dep}\n"));
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| s.to_string()).collect()
    }

    fn app(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("wisp-recipe-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("add/sqlite/src")).unwrap();
        fs::write(
            root.join("Cargo.toml"),
            "[package]\nname = \"a\"\n\n[dependencies]\nwisp = \"1\"\n",
        )
        .unwrap();
        fs::write(root.join("add/sqlite/src/db.rs"), "pub fn open() {}\n").unwrap();
        fs::write(
            root.join("add/sqlite/recipe"),
            "# sqlx\ndep sqlx = { version = \"0.8\", features = [\"sqlite\"] }\nenv DATABASE_URL=sqlite://app.db\nfile src/db.rs\nnote Call db::open().\n",
        )
        .unwrap();
        root
    }

    #[test]
    fn a_recipe_applies_once() {
        let root = app("once");
        assert!(is_recipe(&root, &args(&["sqlite"])));
        assert!(!is_recipe(&root, &args(&["canvas-confetti"])));
        assert!(!is_recipe(&root, &args(&["../x"])));
        run(&root, &args(&["sqlite"])).unwrap();
        let cargo = fs::read_to_string(root.join("Cargo.toml")).unwrap();
        assert!(cargo.contains("[dependencies]\nsqlx = {"), "{cargo}");
        assert_eq!(
            fs::read_to_string(root.join(".env.example")).unwrap(),
            "DATABASE_URL=sqlite://app.db\n"
        );
        // Again: nothing doubles, the app's own file stays; --force replaces it.
        fs::write(root.join("src/db.rs"), "mine").unwrap();
        run(&root, &args(&["sqlite"])).unwrap();
        assert_eq!(fs::read_to_string(root.join("Cargo.toml")).unwrap(), cargo);
        assert_eq!(
            fs::read_to_string(root.join(".env.example"))
                .unwrap()
                .lines()
                .count(),
            1
        );
        assert_eq!(fs::read_to_string(root.join("src/db.rs")).unwrap(), "mine");
        run(&root, &args(&["sqlite", "--force"])).unwrap();
        assert_eq!(
            fs::read_to_string(root.join("src/db.rs")).unwrap(),
            "pub fn open() {}\n"
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn a_bad_recipe_writes_nothing() {
        let root = app("bad");
        let cargo = fs::read_to_string(root.join("Cargo.toml")).unwrap();
        for line in [
            "file ../evil",
            "file /etc/passwd",
            "file src/none.rs",
            "run rm -rf",
            "dep 3",
            "env =x",
        ] {
            fs::write(
                root.join("add/sqlite/recipe"),
                format!("dep a = \"1\"\n{line}\n"),
            )
            .unwrap();
            assert!(run(&root, &args(&["sqlite"])).is_err(), "{line}");
        }
        assert_eq!(fs::read_to_string(root.join("Cargo.toml")).unwrap(), cargo);
        assert!(run(&root, &args(&["nope"])).is_err());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn deps_by_crate() {
        assert_eq!(dep_name("sqlx = \"0.8\""), Some("sqlx"));
        assert_eq!(dep_name("tokio.workspace = true"), Some("tokio"));
        assert_eq!(dep_name("sqlx"), None);
        assert_eq!(
            with_dep("[package]\n", "a = \"1\"").unwrap(),
            "[package]\n\n[dependencies]\na = \"1\"\n"
        );
        assert!(with_dep("[dependencies]\na.workspace = true\n", "a = \"1\"").is_none());
    }
}
