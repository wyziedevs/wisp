//! `wisp check --types`: TypeScript's own checker over the app's
//! TypeScript (`<script lang="ts">`, `src/lib/*.ts`, `+page.ts`). The build
//! only strips types; this checks them, when Node and the app's
//! `typescript` package are there. Without them it says so and skips.
//! Server values are typed by the compiler: see [`probe`].

use crate::{cargo, term};
use std::path::{Path, PathBuf};
use std::process::Command;
use wisp_shared::json::{self, Json};

/// Where the files for tsc go, under the app.
const DIR: &str = ".wisp/types";

const TSCONFIG: &str = r#"{
  "compilerOptions": {
    "target": "es2022",
    "module": "esnext",
    "moduleResolution": "bundler",
    "lib": ["es2022", "dom", "dom.iterable"],
    "strict": true,
    "noEmit": true,
    "skipLibCheck": true,
    "allowJs": true,
    "allowImportingTsExtensions": true,
    "paths": { "$lib/*": ["../../src/lib/*"] }
  },
  "include": ["**/*.ts", "../../src/lib/**/*.ts", "../../src/routes/**/+page.ts"]
}
"#;

pub fn check(root: &Path) -> Result<(), String> {
    let tsc = std::env::var_os("WISP_TSC")
        .map(PathBuf::from)
        .unwrap_or_else(|| root.join("node_modules/typescript/bin/tsc"));
    if !tsc.exists() {
        term::warn(&format!(
            "Types not checked: TypeScript is not installed ({}).\n    Install it with npm install -D typescript, or set WISP_TSC to its bin/tsc.",
            tsc.display()
        ));
        return Ok(());
    }
    let (mut files, wanted) = wisp_build::types(root, "")?;
    if wanted {
        files = wisp_build::types(root, &probe(root)?)?.0;
    }
    let dir = root.join(DIR);
    let _ = std::fs::remove_dir_all(&dir);
    for (rel, text) in files
        .iter()
        .chain([&("tsconfig.json".to_string(), TSCONFIG.to_string())])
    {
        let path = dir.join(rel);
        if let Some(parent) = path.parent() {
            crate::make_dir(parent)?;
        }
        std::fs::write(&path, text)
            .map_err(|e| format!("Could not write {}: {e}", path.display()))?;
    }
    let run = Command::new("node")
        .arg(&tsc)
        .args(["-p", DIR, "--pretty", "false"])
        .current_dir(root)
        .output();
    let out = match run {
        Ok(out) => out,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            term::warn("Types not checked: Node is not installed (https://nodejs.org).");
            return Ok(());
        }
        Err(e) => return Err(format!("Could not run node: {e}")),
    };
    if out.status.success() {
        term::done("Types are valid.");
        return Ok(());
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let lines: Vec<String> = text.lines().map(at_source).collect();
    Err(format!("TypeScript found errors:\n{}", lines.join("\n")))
}

/// The types of the values scripts read of blocks, as the compiler infers
/// them: the app built with `wisp/types` prints them (see `wisp::ts`) and
/// runs nothing else. A value of a type with no TypeScript is `unknown`.
fn probe(root: &Path) -> Result<String, String> {
    let b = cargo::build_for(root, false, false, &["--features", "wisp/types"], &[]);
    let exe = b.exe.filter(|_| b.ok).ok_or_else(|| {
        format!(
            "The app does not compile, so its values have no types:\n{}",
            b.errors
        )
    })?;
    let out = Command::new(&exe)
        .env("WISP_TYPES", "1")
        .current_dir(root)
        .output()
        .map_err(|e| format!("Could not run {}: {e}", exe.display()))?;
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    if !out.status.success() || !text.starts_with('{') {
        return Err(format!(
            "The app did not print its types:\n{text}{}",
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    if let Ok(Json::Obj(files)) = json::parse(&text) {
        for (file, f) in &files {
            let values = f.get("values").into_iter().flat_map(Json::items);
            for v in values.filter(|v| v.items().nth(1).and_then(Json::as_str) == Some("unknown")) {
                let name = v.items().next().and_then(Json::as_str).unwrap_or("?");
                term::warn(&format!(
                    "{file}: `{name}` is `unknown` to TypeScript: its type has none.\n    #[derive(Json)] on it gives it one."
                ));
            }
        }
    }
    Ok(text)
}

/// A tsc line, `.wisp/types/src/routes/+page.wisp.ts(5,3): error TS2322: …`,
/// as `src/routes/+page.wisp:5:3: error TS2322: …`: its scripts are on
/// their lines of the file.
fn at_source(line: &str) -> String {
    let Some(i) = line.find("): ") else {
        return line.to_string();
    };
    let Some(j) = line[..i].rfind('(') else {
        return line.to_string();
    };
    let Some((l, c)) = line[j + 1..i].split_once(',') else {
        return line.to_string();
    };
    let path = line[..j].replace('\\', "/");
    let path = path.strip_prefix(&format!("{DIR}/")).unwrap_or(&path);
    let path = path
        .strip_suffix(".wisp.ts")
        .map_or(path.to_string(), |p| format!("{p}.wisp"));
    format!("{path}:{l}:{c}: {}", &line[i + 3..])
}

#[cfg(test)]
mod tests {
    #[test]
    fn errors_point_at_the_wisp_file() {
        assert_eq!(
            super::at_source(
                ".wisp/types/src/routes/+page.wisp.ts(5,3): error TS2322: Type 'string' is not assignable to type 'number'."
            ),
            "src/routes/+page.wisp:5:3: error TS2322: Type 'string' is not assignable to type 'number'."
        );
        assert_eq!(
            super::at_source("src/lib/util.ts(2,10): error TS2304: Cannot find name 'x'."),
            "src/lib/util.ts:2:10: error TS2304: Cannot find name 'x'."
        );
        assert_eq!(super::at_source("Found 2 errors."), "Found 2 errors.");
    }
}
