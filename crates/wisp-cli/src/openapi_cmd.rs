//! `wisp openapi` prints the app's OpenAPI 3.1 document, the one the build
//! makes and `/_wisp/openapi.json` serves; `-o` writes it to a file, and
//! `--check` fails when that file is not what the app describes now, for CI.

use std::path::Path;

use crate::term;

/// The file `--check` reads when `-o` names none.
const DEFAULT: &str = "openapi.json";

const USAGE: &str = "wisp openapi takes -o <file> and --check: wisp openapi, wisp openapi -o openapi.json, wisp openapi --check [-o openapi.json].";

/// What `wisp openapi` was asked.
#[derive(Debug, PartialEq)]
struct Options {
    out: Option<String>,
    check: bool,
}

fn options(args: &[String]) -> Result<Options, String> {
    let mut o = Options {
        out: None,
        check: false,
    };
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--check" => o.check = true,
            "-o" | "--out" => match it.next() {
                Some(file) if !file.starts_with('-') => o.out = Some(file.clone()),
                _ => return Err(format!("{a} takes a file.\n{USAGE}")),
            },
            other => return Err(format!("Unexpected {other}.\n{USAGE}")),
        }
    }
    Ok(o)
}

pub fn run(root: &Path, args: &[String]) -> Result<(), String> {
    let o = options(args)?;
    crate::cargo::warn_if_stale(root);
    let json = wisp_build::openapi(root)?;
    if json.is_empty() {
        return Err(
            "The app has no endpoints (+server.rs files) or form actions to describe.".into(),
        );
    }
    let doc = wisp_build::pretty_json(&json);
    let file = o.out.as_deref().unwrap_or(DEFAULT);
    if o.check {
        return match std::fs::read_to_string(root.join(file)) {
            Ok(have) if have.replace("\r\n", "\n") == doc => {
                term::done(&format!("{file} is up to date."));
                Ok(())
            }
            Ok(_) => Err(format!(
                "{file} is out of date.\nRun wisp openapi -o {file} and commit it."
            )),
            Err(_) => Err(format!(
                "There is no {file} to check.\nRun wisp openapi -o {file} and commit it."
            )),
        };
    }
    match o.out {
        Some(file) => {
            let path = root.join(&file);
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir)
                    .map_err(|e| format!("Could not create {}: {e}", dir.display()))?;
            }
            std::fs::write(&path, doc).map_err(|e| format!("Could not write {file}: {e}"))?;
            term::done(&format!("Wrote {file}"));
        }
        None => print!("{doc}"),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opts(s: &str) -> Result<Options, String> {
        options(&s.split_whitespace().map(String::from).collect::<Vec<_>>())
    }

    #[test]
    fn options_are_a_file_and_check() {
        assert_eq!(
            opts("").unwrap(),
            Options {
                out: None,
                check: false
            }
        );
        let o = opts("--check -o spec/api.json").unwrap();
        assert_eq!((o.out.as_deref(), o.check), (Some("spec/api.json"), true));
        for bad in ["-o", "-o --check", "--json", "x"] {
            assert!(opts(bad).is_err(), "{bad}");
        }
    }
}
