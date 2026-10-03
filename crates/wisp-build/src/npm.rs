//! npm packages. `package.json`'s `dependencies` name them and pin their
//! versions (`wisp add` writes it); a bare import (`'canvas-confetti'`,
//! `'pkg/sub'`, `'@scope/pkg'`) loads the package's ES module build from
//! esm.sh in dev. A release build serves the copy `wisp build` downloaded
//! into `.wisp/npm` instead, the same files under `/_app/c/npm/`, so
//! production needs no CDN. No Node either way.
//!
//! `wisp build` points each module's imports at `/_app/c/npm/` as it
//! downloads it, so the app's build embeds the files as they are. esm.sh
//! answers a package's address with a stub that only re-exports the
//! package's build (`export * from "/x@1/es2022/x.mjs"`): imports skip it.

use crate::js;
use crate::protocol::NPM_MODULES;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use wisp_shared::json::{self, Json};

pub const ESM: &str = "https://esm.sh";

/// The app's packages, and where its imports of them load from.
pub(crate) struct Npm {
    deps: Vec<(String, String)>,
    /// A release build: `.wisp/npm`, which the imports load.
    vendor: Option<PathBuf>,
}

impl Npm {
    pub(crate) fn new(deps: Vec<(String, String)>, vendor: Option<PathBuf>) -> Npm {
        Npm { deps, vendor }
    }

    /// The URL a bare import loads; an error for a package package.json
    /// does not list.
    pub(crate) fn url(&self, spec: &str) -> Result<String, String> {
        let (pkg, sub) = split(spec).ok_or_else(|| format!("`{spec}` is not a package name"))?;
        let Some((_, ver)) = self.deps.iter().find(|(n, _)| n == pkg) else {
            return Err(format!(
                "`{spec}` is not one of the app's packages (package.json); `wisp add {pkg}` adds it"
            ));
        };
        let path = esm_path(pkg, ver, sub);
        let Some(dir) = &self.vendor else {
            return Ok(format!("{ESM}{path}"));
        };
        let file = file(&path)?;
        let src = crate::read_source(&dir.join(&file)).map_err(|_| absent(&path))?;
        Ok(stub_target(&src).unwrap_or_else(|| format!("{NPM_MODULES}{file}")))
    }
}

pub(crate) fn absent(path: &str) -> String {
    format!("{ESM}{path} is not in .wisp/npm; `wisp build` downloads the app's packages there")
}

/// Whether an import names a package: `pkg`, `@scope/pkg`, `pkg/sub`; not
/// a path, a URL (`https:`, `data:`) or `$lib/`.
pub(crate) fn is_bare(spec: &str) -> bool {
    spec.starts_with(|c: char| c.is_ascii_alphanumeric() || c == '@') && !spec.contains(':')
}

/// A bare import's package and subpath: `@scope/pkg/a/b` is
/// (`@scope/pkg`, `/a/b`), `pkg` is (`pkg`, ``).
pub fn split(spec: &str) -> Option<(&str, &str)> {
    let first = spec.find('/');
    let end = match first {
        Some(i) if spec.starts_with('@') => spec[i + 1..].find('/').map(|j| i + 1 + j),
        _ => first,
    };
    let pkg = end.map_or(spec, |e| &spec[..e]);
    valid_name(pkg).then(|| (pkg, &spec[pkg.len()..]))
}

/// An npm package name: lower case letters, digits and `-._~`, with one
/// `@scope/` before it if scoped.
pub fn valid_name(name: &str) -> bool {
    let part = |p: &str| {
        !p.is_empty()
            && !p.starts_with('.')
            && p.bytes().all(|b| {
                b.is_ascii_lowercase()
                    || b.is_ascii_digit()
                    || matches!(b, b'-' | b'.' | b'_' | b'~')
            })
    };
    match name.strip_prefix('@') {
        Some(scoped) => scoped
            .split_once('/')
            .is_some_and(|(s, n)| part(s) && part(n)),
        None => name.len() <= 214 && part(name),
    }
}

/// An exact version, like `1.2.3` or `2.0.0-beta.1`: what `wisp add` pins.
pub fn exact(version: &str) -> bool {
    let core = version.split(['-', '+']).next().unwrap_or("");
    let nums: Vec<&str> = core.split('.').collect();
    nums.len() == 3
        && nums
            .iter()
            .all(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
        && version
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'+'))
}

/// The esm.sh path of a package's module: its ES2022 build, whose own
/// dependencies esm.sh resolves and serves as modules of their own.
pub fn esm_path(pkg: &str, ver: &str, sub: &str) -> String {
    format!("/{pkg}@{ver}{sub}?target=es2022")
}

/// The file under `.wisp/npm` that holds the module at an esm.sh path,
/// and its path under `/_app/c/npm/`. A module's own path
/// (`/x@1/es2022/x.mjs`) is its file (`x@1/es2022/x.mjs`), so a file
/// names its module; a stub's has its query joined on with `_`, any
/// character a file name may not have as `_`, and `.js`.
pub fn file(path: &str) -> Result<String, String> {
    let bad = || format!("{ESM}{path} is not a module path wisp can keep");
    let (p, q) = path.split_once('?').unwrap_or((path, ""));
    let clean = |s: &str, out: &mut String| {
        out.extend(s.chars().map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | '@' | '+' | '~') {
                c
            } else {
                '_'
            }
        }))
    };
    let mut out = String::with_capacity(path.len() + 3);
    for seg in p.strip_prefix('/').ok_or_else(bad)?.split('/') {
        if matches!(seg, "" | "." | "..") {
            return Err(bad());
        }
        if !out.is_empty() {
            out.push('/');
        }
        clean(seg, &mut out);
    }
    if !q.is_empty() {
        out.push('_');
        clean(q, &mut out);
        out.push_str(".js");
    }
    Ok(out)
}

/// A module's own path, which is its file's: no query, no character a
/// file name may not have.
fn own(path: &str) -> bool {
    file(path).is_ok_and(|f| path[1..] == f)
}

/// The module a stub re-exports, if `src` is one: nothing but `import
/// "…"` (what that module imports, for the browser to fetch early),
/// `export * from "…"` and `export { default } from "…"` of one module.
pub fn stub_target(src: &str) -> Option<String> {
    let t = js::tokens(src);
    let words: Vec<&str> = t.iter().map(|t| t.text(src)).collect();
    let quoted = |s: &str| s.len() >= 2 && (s.starts_with('"') || s.starts_with('\''));
    let mut target: Option<&str> = None;
    let mut w = &words[..];
    while !w.is_empty() {
        let (spec, n) = match w {
            [";", ..] => (None, 1),
            ["import", s, ..] if quoted(s) => (None, 2),
            ["export", "*", "from", s, ..] => (Some(*s), 4),
            ["export", "{", "default", "}", "from", s, ..] => (Some(*s), 6),
            _ => return None,
        };
        if let Some(s) = spec {
            if !quoted(s) || target.is_some_and(|t| t != s) {
                return None;
            }
            target = Some(s);
        }
        w = &w[n..];
    }
    target.map(|s| s[1..s.len() - 1].to_string())
}

/// The module esm.sh served at `path`, its imports of esm.sh modules
/// (`/x@1/es2022/x.mjs`, a full URL, `./a.mjs`) pointed at `/_app/c/npm/`.
/// `fetch` gives what esm.sh serves at a path that is not a module's own
/// (`/y@^2?target=es2022`): a stub, whose module is imported instead.
pub fn rewrite(
    path: &str,
    src: &str,
    mut fetch: impl FnMut(&str) -> Result<String, String>,
) -> Result<String, String> {
    let base = path.split_once('?').map_or(path, |(p, _)| p);
    let dir = &base[..base.rfind('/').unwrap_or(0)];
    js::specifiers(src, |s| {
        let s = s.strip_prefix(ESM).unwrap_or(s);
        let abs = if s.starts_with("./") || s.starts_with("../") {
            let mut parts: Vec<&str> = dir.split('/').filter(|p| !p.is_empty()).collect();
            for p in s.split('/') {
                match p {
                    "." => {}
                    ".." => {
                        parts.pop();
                    }
                    p => parts.push(p),
                }
            }
            format!("/{}", parts.join("/"))
        } else if s.starts_with('/') && !s.starts_with("//") {
            s.to_string()
        } else {
            return Ok(None);
        };
        let target = if own(&abs) {
            Some(abs.clone())
        } else {
            stub_target(&fetch(&abs)?).filter(|t| own(t))
        };
        match target {
            Some(t) => Ok(Some(format!("{NPM_MODULES}{}", &t[1..]))),
            None => Err(format!(
                "{ESM}{abs}, which {ESM}{path} imports, is not a module wisp can keep"
            )),
        }
    })
}

/// The modules of `.wisp/npm` that the modules at `roots` (esm.sh paths)
/// reach, themselves included, as file and text. They are read a level of
/// imports at a time; `missing` is given the paths of a level that are not
/// there, to put there (`wisp build` downloads them).
pub fn walk(
    dir: &Path,
    roots: &[String],
    mut missing: impl FnMut(&[String]) -> Result<(), String>,
) -> Result<Vec<(String, String)>, String> {
    let mut seen: BTreeSet<String> = roots.iter().cloned().collect();
    let mut level: Vec<String> = seen.iter().cloned().collect();
    let mut out = Vec::new();
    while !level.is_empty() {
        let files = level
            .iter()
            .map(|p| file(p))
            .collect::<Result<Vec<_>, _>>()?;
        let absent: Vec<String> = level
            .iter()
            .zip(&files)
            .filter(|(_, f)| !dir.join(f).is_file())
            .map(|(p, _)| p.clone())
            .collect();
        if !absent.is_empty() {
            missing(&absent)?;
        }
        let mut next = Vec::new();
        for (path, f) in level.iter().zip(files) {
            let src = crate::read_source(&dir.join(&f)).map_err(|_| self::absent(path))?;
            js::specifiers(&src, |s| {
                if let Some(rest) = s.strip_prefix(NPM_MODULES)
                    && seen.insert(format!("/{rest}"))
                {
                    next.push(format!("/{rest}"));
                }
                Ok(None)
            })?;
            out.push((f, src));
        }
        level = next;
    }
    Ok(out)
}

/// package.json's `dependencies` (none without one), each an exact version.
pub fn deps(root: &Path) -> Result<Vec<(String, String)>, String> {
    let Ok(text) = crate::read_source(&root.join("package.json")) else {
        return Ok(Vec::new());
    };
    let doc = json::parse(&text).map_err(|e| format!("package.json: {e}"))?;
    let deps = match doc.get("dependencies") {
        None => return Ok(Vec::new()),
        Some(Json::Obj(deps)) => deps,
        Some(_) => return Err("package.json: `dependencies` is not an object".into()),
    };
    deps.iter()
        .map(|(n, v)| match v.as_str() {
            Some(v) if exact(v) => Ok((n.clone(), v.to_string())),
            Some(v) => Err(format!(
                "package.json: `{n}` is `{v}`, not an exact version; `wisp add {n}@1.2.3` pins one (`wisp add {n}`, the latest)"
            )),
            None => Err(format!(
                "package.json: the version of `{n}` is not a string"
            )),
        })
        .collect()
}

/// package.json's text (`None`: there is none yet) with `name` pinned to
/// `version` in `dependencies`, or taken out of it for `None`. The rest
/// stays; packages are kept in name order, as npm keeps them.
pub fn set_dep(text: Option<&str>, name: &str, version: Option<&str>) -> Result<String, String> {
    let mut doc = match text {
        Some(t) => json::parse(t).map_err(|e| format!("package.json: {e}"))?,
        None => Json::Obj(Vec::new()),
    };
    let Json::Obj(top) = &mut doc else {
        return Err("package.json: it is not an object".into());
    };
    if !top.iter().any(|(k, _)| k == "dependencies") {
        top.push(("dependencies".into(), Json::Obj(Vec::new())));
    }
    let Some((_, Json::Obj(deps))) = top.iter_mut().find(|(k, _)| k == "dependencies") else {
        return Err("package.json: `dependencies` is not an object".into());
    };
    let at = deps.iter().position(|(k, _)| k == name);
    match (version, at) {
        (Some(v), Some(i)) => deps[i].1 = Json::Str(v.into()),
        (Some(v), None) => deps.push((name.into(), Json::Str(v.into()))),
        (None, Some(i)) => {
            deps.remove(i);
        }
        (None, None) => return Err(format!("{name} is not in package.json")),
    }
    deps.sort_by(|a, b| a.0.cmp(&b.0));
    let mut out = String::new();
    write(&doc, 0, &mut out);
    out.push('\n');
    Ok(out)
}

/// package.json as npm writes it (two spaces), every member in its order.
fn write(v: &Json, depth: usize, out: &mut String) {
    let pad = |out: &mut String, d: usize| out.extend(std::iter::repeat_n("  ", d));
    match v {
        Json::Null => out.push_str("null"),
        Json::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Json::Num(n) => out.push_str(n),
        Json::Str(s) => out.push_str(&crate::json_str(s)),
        Json::Arr(items) if items.is_empty() => out.push_str("[]"),
        Json::Obj(members) if members.is_empty() => out.push_str("{}"),
        Json::Arr(items) => {
            out.push_str("[\n");
            for (k, item) in items.iter().enumerate() {
                pad(out, depth + 1);
                write(item, depth + 1, out);
                out.push_str(if k + 1 < items.len() { ",\n" } else { "\n" });
            }
            pad(out, depth);
            out.push(']');
        }
        Json::Obj(members) => {
            out.push_str("{\n");
            for (k, (key, item)) in members.iter().enumerate() {
                pad(out, depth + 1);
                out.push_str(&crate::json_str(key));
                out.push_str(": ");
                write(item, depth + 1, out);
                out.push_str(if k + 1 < members.len() { ",\n" } else { "\n" });
            }
            pad(out, depth);
            out.push('}');
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        assert_eq!(split("canvas-confetti"), Some(("canvas-confetti", "")));
        assert_eq!(split("pkg/a/b.js"), Some(("pkg", "/a/b.js")));
        assert_eq!(split("@scope/pkg"), Some(("@scope/pkg", "")));
        assert_eq!(split("@scope/pkg/sub"), Some(("@scope/pkg", "/sub")));
        for bad in ["@scope", "Pkg", "@/x", ".x", "a b"] {
            assert_eq!(split(bad), None, "{bad}");
        }
        assert!(is_bare("x") && is_bare("@s/x") && is_bare("x/y"));
        for not in [
            "./x",
            "/x",
            "$lib/x.js",
            "https://esm.sh/x",
            "data:text/javascript,",
        ] {
            assert!(!is_bare(not), "{not}");
        }
        for v in ["1.2.3", "0.0.1-beta.1", "2.0.0+build.5"] {
            assert!(exact(v), "{v}");
        }
        for v in ["^1.0", "1.2", "~1.2.3", "1.x.0", "latest", "1.2.3 || 2", ""] {
            assert!(!exact(v), "{v}");
        }
    }

    #[test]
    fn paths() {
        assert_eq!(
            file("/canvas-confetti@1.9.3?target=es2022").unwrap(),
            "canvas-confetti@1.9.3_target_es2022.js"
        );
        assert_eq!(file("/x@1/es2022/x.mjs").unwrap(), "x@1/es2022/x.mjs");
        assert_eq!(
            file("/@s/core@^1.7.0?target=es2022").unwrap(),
            "@s/core@_1.7.0_target_es2022.js"
        );
        for bad in ["x", "/../x", "/a//b", "/a/./b"] {
            assert!(file(bad).is_err(), "{bad}");
        }
        assert!(own("/x@1/es2022/x.mjs") && !own("/x@1?target=es2022") && !own("/y@^2/y.mjs"));
    }

    #[test]
    fn stubs() {
        let stub = "/* esm.sh - x@1 */\nimport \"/y@2/es2022/y.mjs\";\nexport * from \"/x@1/es2022/x.mjs\";\nexport { default } from \"/x@1/es2022/x.mjs\";\n";
        assert_eq!(stub_target(stub).as_deref(), Some("/x@1/es2022/x.mjs"));
        for not in [
            "export * from \"/a.mjs\";\nexport * from \"/b.mjs\";",
            "import \"/a.mjs\";",
            "export * from \"/a.mjs\";\nconsole.log(1)",
            "import x from \"/a.mjs\";\nexport default x;",
        ] {
            assert_eq!(stub_target(not), None, "{not}");
        }
    }

    #[test]
    fn imports_point_at_the_app() {
        let src = "import \"/a@1/es2022/a.mjs\";\nexport * from \"https://esm.sh/b@2/es2022/b.mjs\";\nimport x from \"./c.mjs\"\nimport y from \"../d@1/x.mjs\"\nconst s = import(\"/s@^1?target=es2022\")\nimport z from \"https://example.com/z.js\"";
        let mut asked = Vec::new();
        let out = rewrite("/p@1/es2022/p.mjs", src, |p| {
            asked.push(p.to_string());
            Ok("export * from \"/s@1.2.0/es2022/s.mjs\";".into())
        })
        .unwrap();
        assert_eq!(
            out,
            "import \"/_app/c/npm/a@1/es2022/a.mjs\";\nexport * from \"/_app/c/npm/b@2/es2022/b.mjs\";\nimport x from \"/_app/c/npm/p@1/es2022/c.mjs\"\nimport y from \"/_app/c/npm/p@1/d@1/x.mjs\"\nconst s = import(\"/_app/c/npm/s@1.2.0/es2022/s.mjs\")\nimport z from \"https://example.com/z.js\""
        );
        assert_eq!(asked, ["/s@^1?target=es2022"]);
        let err = rewrite("/p@1/p.mjs", "import \"/q?x\"", |_| Ok("let q".into())).unwrap_err();
        assert!(err.contains("is not a module wisp can keep"), "{err}");
    }

    #[test]
    fn urls() {
        let deps = vec![("canvas-confetti".to_string(), "1.9.3".to_string())];
        let dev = Npm::new(deps, None);
        assert_eq!(
            dev.url("canvas-confetti").unwrap(),
            "https://esm.sh/canvas-confetti@1.9.3?target=es2022"
        );
        let err = dev.url("left-pad").unwrap_err();
        assert!(err.contains("wisp add left-pad"), "{err}");
    }

    #[test]
    fn the_walk_reads_what_is_there() {
        let dir = std::env::temp_dir().join(format!("wisp-npm-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("x@1/es2022")).unwrap();
        std::fs::write(
            dir.join("x@1_target_es2022.js"),
            "export * from \"/_app/c/npm/x@1/es2022/x.mjs\";",
        )
        .unwrap();
        // A release build imports the stub's module, from .wisp/npm.
        let npm = |v: &str| Npm::new(vec![("x".into(), v.into())], Some(dir.clone()));
        assert!(
            npm("2")
                .url("x")
                .unwrap_err()
                .contains("is not in .wisp/npm")
        );
        assert_eq!(npm("1").url("x").unwrap(), "/_app/c/npm/x@1/es2022/x.mjs");
        let roots = ["/x@1?target=es2022".to_string()];
        let mut asked = Vec::new();
        let err = walk(&dir, &roots, |p| {
            asked.extend_from_slice(p);
            Err("offline".into())
        })
        .unwrap_err();
        assert_eq!(err, "offline");
        assert_eq!(asked, ["/x@1/es2022/x.mjs"]);
        let files = walk(&dir, &roots, |p| {
            for p in p {
                std::fs::write(dir.join(file(p).unwrap()), "export default 1;").unwrap();
            }
            Ok(())
        })
        .unwrap();
        let again = walk(&dir, &roots, |_| Err("fetched again".into()));
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(Ok(&files), again.as_ref());
        assert_eq!(files.len(), 2);
        assert_eq!(
            files[1],
            ("x@1/es2022/x.mjs".into(), "export default 1;".into())
        );
    }

    #[test]
    fn package_json() {
        let text = "{\n  \"name\": \"app\",\n  \"version\": 1.5e0,\n  \"scripts\": {\"a\": \"b\\u00e9\"},\n  \"files\": [],\n  \"dependencies\": {\"zed\": \"1.0.0\"}\n}\n";
        let added = set_dep(Some(text), "@a/b", Some("2.0.0")).unwrap();
        assert_eq!(
            added,
            "{\n  \"name\": \"app\",\n  \"version\": 1.5e0,\n  \"scripts\": {\n    \"a\": \"bé\"\n  },\n  \"files\": [],\n  \"dependencies\": {\n    \"@a/b\": \"2.0.0\",\n    \"zed\": \"1.0.0\"\n  }\n}\n"
        );
        let root = std::env::temp_dir().join(format!("wisp-pkg-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        // With a byte order mark, as some editors save it.
        std::fs::write(root.join("package.json"), format!("\u{feff}{added}")).unwrap();
        let got = deps(&root);
        std::fs::write(
            root.join("package.json"),
            "{\"dependencies\": {\"x\": \"^1.0\"}}",
        )
        .unwrap();
        let range = deps(&root);
        let _ = std::fs::remove_dir_all(&root);
        assert_eq!(
            got.unwrap(),
            [
                ("@a/b".into(), "2.0.0".into()),
                ("zed".into(), "1.0.0".into())
            ]
        );
        assert!(range.unwrap_err().contains("`wisp add x@1.2.3`"));
        let removed = set_dep(Some(&added), "zed", None).unwrap();
        assert!(!removed.contains("zed") && removed.contains("@a/b"));
        assert!(set_dep(Some(&removed), "zed", None).is_err());
        assert_eq!(
            set_dep(None, "x", Some("1")).unwrap(),
            "{\n  \"dependencies\": {\n    \"x\": \"1\"\n  }\n}\n"
        );
        for bad in ["[]", "{", "{\"a\" 1}", "{\"dependencies\": 1}", "{} x"] {
            assert!(set_dep(Some(bad), "x", Some("1")).is_err(), "{bad}");
        }
    }
}
