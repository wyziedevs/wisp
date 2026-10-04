//! `wisp ui add` and `wisp ui list`: accessible components, copied into
//! the app's `src/components`. The app owns the
//! copy: change it as you like; Wisp never touches it again.

use crate::term;
use std::fs;
use std::io::{self, Write};
use std::path::Path;

/// A component: its name, what it is and its file.
type Part = (&'static str, &'static str, &'static str);

macro_rules! part {
    ($name:literal, $about:literal) => {
        (
            $name,
            $about,
            include_str!(concat!("../templates/ui/", $name, ".wisp")),
        )
    };
}

const PARTS: [Part; 14] = [
    part!("Accordion", "A section that opens and closes (<details>)."),
    part!(
        "Badge",
        "A small label: neutral, accent, success, warning, danger."
    ),
    part!(
        "Button",
        "A button, or a link that looks like one, in four variants."
    ),
    part!("Card", "A panel with an optional title."),
    part!("Checkbox", "A checkbox with its label."),
    part!("Dialog", "A modal <dialog> and the button that opens it."),
    part!("Input", "A text field with its label, hint and problem."),
    part!("Menu", "A menu button: a popover list, arrow keys to move."),
    part!("Select", "A native <select> with its label."),
    part!("Switch", "An on/off checkbox (role=\"switch\")."),
    part!("Tabs", "Tabs over panels, arrow keys to move."),
    part!("Textarea", "A text area with its label, hint and problem."),
    part!(
        "Toast",
        "Short messages in a corner, read out by screen readers."
    ),
    part!("Tooltip", "Text over an element on hover and focus."),
];

/// `wisp ui add <name…> [--force]` and `wisp ui list`.
pub fn run(
    root: impl FnOnce() -> Result<&'static Path, String>,
    args: &[String],
) -> Result<(), String> {
    match args.first().map(String::as_str) {
        Some("list") if args.len() == 1 => {
            list();
            Ok(())
        }
        Some("add") => {
            let force = args[1..].iter().any(|a| a == "--force");
            let names: Vec<&str> = args[1..]
                .iter()
                .map(String::as_str)
                .filter(|a| *a != "--force")
                .collect();
            add(root()?, &names, force)
        }
        _ => Err("wisp ui takes add or list: wisp ui add button dialog, or wisp ui list.".into()),
    }
}

fn list() {
    let width = PARTS.iter().map(|p| p.0.len()).max().unwrap_or(0);
    for (name, about, ..) in PARTS {
        println!(
            "  {}  {about}",
            term::accent(&format!("{:width$}", name.to_ascii_lowercase()))
        );
    }
    println!("\n  wisp ui add button dialog   (--force writes over the app's own copy)");
}

/// The component of a name, in any case: `dialog`, `Dialog`.
fn find(name: &str) -> Result<&'static Part, String> {
    PARTS
        .iter()
        .find(|p| p.0.eq_ignore_ascii_case(name))
        .ok_or_else(|| {
            let names: Vec<String> = PARTS.iter().map(|p| p.0.to_ascii_lowercase()).collect();
            format!(
                "There is no component {name}.\nThere are: {}.",
                names.join(", ")
            )
        })
}

/// Each component into `src/components`. A file there
/// already is the app's: it stays, unless `force`. Every name is checked
/// before anything is written.
fn add(root: &Path, names: &[&str], force: bool) -> Result<(), String> {
    if names.is_empty() {
        return Err("wisp ui add takes components, like wisp ui add button dialog.\nwisp ui list names them.".into());
    }
    let parts = names
        .iter()
        .map(|n| find(n))
        .collect::<Result<Vec<_>, _>>()?;
    let dir = root.join("src").join("components");
    crate::make_dir(&dir)?;
    let mut kept = Vec::new();
    for (name, _, wisp) in parts {
        let file = format!("{name}.wisp");
        if write(&dir.join(&file), wisp, force)? {
            term::done(&format!("Wrote src/components/{file}"));
        } else {
            kept.push(file);
        }
    }
    if !kept.is_empty() {
        term::warn(&format!(
            "Kept the app's own {}; --force writes over them.",
            kept.join(", ")
        ));
    }
    Ok(())
}

/// Writes `text` at `path`: false if a file is there and not `force`.
fn write(path: &Path, text: &str, force: bool) -> Result<bool, String> {
    let file = if force {
        fs::File::create(path)
    } else {
        match fs::File::create_new(path) {
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => return Ok(false),
            f => f,
        }
    };
    file.and_then(|mut f| f.write_all(text.as_bytes()))
        .map(|()| true)
        .map_err(|e| format!("Could not write {}: {e}.", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every component, in an app: valid, with no
    /// accessibility warnings; a second add keeps the app's copy.
    #[test]
    fn components_check_clean() {
        let root = std::env::temp_dir().join(format!("wisp-ui-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let page = root.join("src/routes/+page.wisp");
        fs::create_dir_all(page.parent().unwrap()).unwrap();
        fs::write(&page, "<h1>Kit</h1>").unwrap();
        let names: Vec<&str> = PARTS.iter().map(|p| p.0).collect();
        add(&root, &names, false).unwrap();
        let (_, warnings) = wisp_build::check(&root).unwrap();
        assert!(warnings.is_empty(), "{warnings:?}");

        let button = root.join("src/components/Button.wisp");
        fs::write(&button, "mine").unwrap();
        add(&root, &["button"], false).unwrap();
        assert_eq!(fs::read_to_string(&button).unwrap(), "mine");
        add(&root, &["BUTTON"], true).unwrap();
        assert_eq!(fs::read_to_string(&button).unwrap(), PARTS[2].2);
        assert!(add(&root, &["button", "nope"], false).is_err());
        let _ = fs::remove_dir_all(&root);
    }
}
