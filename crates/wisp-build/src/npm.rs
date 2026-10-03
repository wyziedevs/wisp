//! npm packages. `package.json`'s `dependencies` name them and pin their
//! versions (`wisp add` writes it); a bare import (`'canvas-confetti'`,
//! `'pkg/sub'`, `'@scope/pkg'`) loads the package's ES module build from
//! esm.sh in dev. A release build serves the copy `wisp build` downloaded
//! into `.wisp/npm` instead, the same files under `/_app/c/npm/`, so
//! production needs no CDN. No Node either way.

use crate::protocol::MODULES;
use crate::{fnv1a, js};
use std::cell::RefCell;
use std::collections::BTreeSet;
use std::path::Path;

pub const ESM: &str = "https://esm.sh";

/// The app's packages, and the esm.sh paths its imports load.
pub(crate) struct Npm {
    deps: Vec<(String, String)>,
    /// A release build: imports load `.wisp/npm`'s files, under this `?v=`.
    vendor: Option<String>,
    used: RefCell<BTreeSet<String>>,
}

impl Npm {
    pub(crate) fn new(deps: Vec<(String, String)>, release: bool) -> Npm {
        // A pinned version's files never change, so the versions are the hash.
        let vendor = release.then(|| {
            let list: String = deps.iter().map(|(n, v)| format!("{n}@{v}\n")).collect();
            format!("{:016x}", fnv1a(list.as_bytes()))
        });
        Npm {
            deps,
            vendor,
            used: RefCell::new(BTreeSet::new()),
        }
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
        let url = match &self.vendor {
            None => format!("{ESM}{path}"),
            Some(v) => format!("{MODULES}npm/{}?v={v}", local(&path)?),
        };
        self.used.borrow_mut().insert(path);
        Ok(url)
    }

    /// The esm.sh paths the app imports, in order.
    pub(crate) fn used(&self) -> Vec<String> {
        self.used.borrow().iter().cloned().collect()
    }

    /// For a release build, the files of `.wisp/npm` its imports reach,
    /// as served: path under `/_app/c/npm/` and text, their own imports
    /// pointed there too.
    pub(crate) fn vendored(&self, root: &Path) -> Result<Vec<(String, String)>, String> {
        let Some(v) = &self.vendor else {
            return Ok(Vec::new());
        };
        let dir = root.join(".wisp").join("npm");
        let mut seen = BTreeSet::new();
        let mut stack = self.used();
        let mut out = Vec::new();
        while let Some(path) = stack.pop() {
            if !seen.insert(path.clone()) {
                continue;
            }
            let file = local(&path)?;
            let src = crate::read_source(&dir.join(&file)).map_err(|_| {
                format!(
                    "{ESM}{path} is not in .wisp/npm; `wisp build` downloads the app's packages there"
                )
            })?;
            let src = js::specifiers(&src, |s| {
                let Some(p) = esm_import(s) else {
                    return Ok(None);
                };
                let url = format!("{MODULES}npm/{}?v={v}", local(&p)?);
                stack.push(p);
                Ok(Some(url))
            })?;
            out.push((format!("{MODULES}npm/{file}"), src));
        }
        Ok(out)
    }
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

/// The esm.sh path of a package's module: its ES2022 build, whose own
/// dependencies esm.sh resolves and serves as modules of their own.
pub fn esm_path(pkg: &str, ver: &str, sub: &str) -> String {
    let mut v = String::new();
    for b in ver.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_' | b'~' | b'+') {
            v.push(b as char);
        } else {
            v.push_str(&format!("%{b:02X}"));
        }
    }
    format!("/{pkg}@{v}{sub}?target=es2022")
}

/// The esm.sh path a downloaded module imports (`/x@1/es2022/x.mjs`, or
/// the same as a full URL), if it is one.
fn esm_import(spec: &str) -> Option<String> {
    let path = spec.strip_prefix(ESM).unwrap_or(spec);
    (path.starts_with('/') && !path.starts_with("//")).then(|| path.to_string())
}

/// The esm.sh paths a downloaded module imports, for `wisp build` to fetch.
pub fn imports(src: &str) -> Vec<String> {
    let mut out = Vec::new();
    let _ = js::specifiers(src, |s| {
        out.extend(esm_import(s));
        Ok(None)
    });
    out
}

/// The file under `.wisp/npm` that holds the module at an esm.sh path:
/// the path, its query joined on with `_`, any character a file name may
/// not have as `_`, and `.js` unless it ends in `.js` or `.mjs`.
pub fn local(path: &str) -> Result<String, String> {
    let bad = || format!("{ESM}{path} is not a module path wisp can keep");
    let (p, q) = path.split_once('?').unwrap_or((path, ""));
    let clean = |s: &str| -> String {
        s.chars()
            .map(|c| match c {
                'a'..='z' | 'A'..='Z' | '0'..='9' | '.' | '-' | '_' | '@' | '+' | '~' => c,
                _ => '_',
            })
            .collect()
    };
    let mut segs = Vec::new();
    for seg in p.strip_prefix('/').ok_or_else(bad)?.split('/') {
        if matches!(seg, "" | "." | "..") {
            return Err(bad());
        }
        segs.push(clean(seg));
    }
    let mut out = segs.join("/");
    if !q.is_empty() {
        out.push('_');
        out.push_str(&clean(q));
    }
    if !(out.ends_with(".js") || out.ends_with(".mjs")) {
        out.push_str(".js");
    }
    Ok(out)
}

/// package.json's `dependencies` (none without one).
pub fn deps(root: &Path) -> Result<Vec<(String, String)>, String> {
    let Ok(text) = crate::read_source(&root.join("package.json")) else {
        return Ok(Vec::new());
    };
    let doc = parse(&text).map_err(|e| format!("package.json: {e}"))?;
    let Json::Obj(top) = doc else {
        return Err("package.json: it is not an object".into());
    };
    let Some((_, deps)) = top.iter().find(|(k, _)| k == "dependencies") else {
        return Ok(Vec::new());
    };
    let Json::Obj(deps) = deps else {
        return Err("package.json: `dependencies` is not an object".into());
    };
    deps.iter()
        .map(|(n, v)| match v {
            Json::Str(v) => Ok((n.clone(), v.clone())),
            _ => Err(format!(
                "package.json: the version of `{n}` is not a string"
            )),
        })
        .collect()
}

/// A string member of a JSON object: the npm registry's `version`.
pub fn string_member(text: &str, key: &str) -> Option<String> {
    let Ok(Json::Obj(members)) = parse(text) else {
        return None;
    };
    members.into_iter().find_map(|(k, v)| match v {
        Json::Str(s) if k == key => Some(s),
        _ => None,
    })
}

/// package.json's text (`None`: there is none yet) with `name` pinned to
/// `version` in `dependencies`, or taken out of it for `None`. The rest
/// stays; packages are kept in name order, as npm keeps them.
pub fn set_dep(text: Option<&str>, name: &str, version: Option<&str>) -> Result<String, String> {
    let mut doc = match text {
        Some(t) => parse(t).map_err(|e| format!("package.json: {e}"))?,
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

/// Just enough JSON for package.json: read it, write it back as npm does
/// (two spaces), every member in its order.
#[derive(Debug, PartialEq)]
enum Json {
    Null,
    Bool(bool),
    /// As written, so it comes back the same.
    Num(String),
    Str(String),
    Arr(Vec<Json>),
    Obj(Vec<(String, Json)>),
}

fn parse(text: &str) -> Result<Json, String> {
    let mut p = Parser {
        s: text.as_bytes(),
        i: 0,
    };
    let v = p.value(0)?;
    p.ws();
    if p.i < p.s.len() {
        return Err(p.err("the end"));
    }
    Ok(v)
}

struct Parser<'a> {
    s: &'a [u8],
    i: usize,
}

impl Parser<'_> {
    fn err(&self, want: &str) -> String {
        let line = self.s[..self.i.min(self.s.len())]
            .iter()
            .filter(|&&b| b == b'\n')
            .count();
        format!("line {}: expected {want}", line + 1)
    }

    fn ws(&mut self) {
        while matches!(self.s.get(self.i), Some(b' ' | b'\t' | b'\n' | b'\r')) {
            self.i += 1;
        }
    }

    fn eat(&mut self, b: u8) -> bool {
        self.ws();
        let hit = self.s.get(self.i) == Some(&b);
        self.i += hit as usize;
        hit
    }

    fn value(&mut self, depth: u32) -> Result<Json, String> {
        if depth > 64 {
            return Err(self.err("less nesting"));
        }
        self.ws();
        let word = |p: &mut Self, w: &str, v: Json| {
            if p.s[p.i..].starts_with(w.as_bytes()) {
                p.i += w.len();
                Ok(v)
            } else {
                Err(p.err("a value"))
            }
        };
        match self.s.get(self.i) {
            Some(b'{') => {
                self.i += 1;
                let mut members = Vec::new();
                if self.eat(b'}') {
                    return Ok(Json::Obj(members));
                }
                loop {
                    self.ws();
                    let k = self.string()?;
                    if !self.eat(b':') {
                        return Err(self.err("`:`"));
                    }
                    members.push((k, self.value(depth + 1)?));
                    if self.eat(b'}') {
                        return Ok(Json::Obj(members));
                    }
                    if !self.eat(b',') {
                        return Err(self.err("`,` or `}`"));
                    }
                }
            }
            Some(b'[') => {
                self.i += 1;
                let mut items = Vec::new();
                if self.eat(b']') {
                    return Ok(Json::Arr(items));
                }
                loop {
                    items.push(self.value(depth + 1)?);
                    if self.eat(b']') {
                        return Ok(Json::Arr(items));
                    }
                    if !self.eat(b',') {
                        return Err(self.err("`,` or `]`"));
                    }
                }
            }
            Some(b'"') => self.string().map(Json::Str),
            Some(b't') => word(self, "true", Json::Bool(true)),
            Some(b'f') => word(self, "false", Json::Bool(false)),
            Some(b'n') => word(self, "null", Json::Null),
            Some(b'-' | b'0'..=b'9') => {
                let start = self.i;
                while matches!(
                    self.s.get(self.i),
                    Some(b'-' | b'+' | b'.' | b'e' | b'E' | b'0'..=b'9')
                ) {
                    self.i += 1;
                }
                let n = String::from_utf8_lossy(&self.s[start..self.i]).into_owned();
                n.parse::<f64>()
                    .map(|_| Json::Num(n))
                    .map_err(|_| self.err("a number"))
            }
            _ => Err(self.err("a value")),
        }
    }

    fn string(&mut self) -> Result<String, String> {
        if self.s.get(self.i) != Some(&b'"') {
            return Err(self.err("a string"));
        }
        self.i += 1;
        let mut out = Vec::new();
        loop {
            let Some(&b) = self.s.get(self.i) else {
                return Err(self.err("`\"`"));
            };
            self.i += 1;
            match b {
                b'"' => break,
                b'\\' => {
                    let Some(&e) = self.s.get(self.i) else {
                        return Err(self.err("an escape"));
                    };
                    self.i += 1;
                    let c = match e {
                        b'"' => '"',
                        b'\\' => '\\',
                        b'/' => '/',
                        b'b' => '\u{8}',
                        b'f' => '\u{c}',
                        b'n' => '\n',
                        b'r' => '\r',
                        b't' => '\t',
                        b'u' => {
                            let mut c = self.hex4()?;
                            if (0xd800..0xdc00).contains(&c) && self.s[self.i..].starts_with(b"\\u")
                            {
                                self.i += 2;
                                let lo = self.hex4()?;
                                c = 0x10000
                                    + ((c - 0xd800) << 10)
                                    + (lo.wrapping_sub(0xdc00) & 0x3ff);
                            }
                            char::from_u32(c).unwrap_or('\u{fffd}')
                        }
                        _ => return Err(self.err("an escape")),
                    };
                    out.extend_from_slice(c.encode_utf8(&mut [0; 4]).as_bytes());
                }
                b => out.push(b),
            }
        }
        String::from_utf8(out).map_err(|_| self.err("UTF-8"))
    }

    fn hex4(&mut self) -> Result<u32, String> {
        let digits = self
            .s
            .get(self.i..self.i + 4)
            .ok_or_else(|| self.err("4 hex digits"))?;
        let n = std::str::from_utf8(digits)
            .ok()
            .and_then(|d| u32::from_str_radix(d, 16).ok())
            .ok_or_else(|| self.err("4 hex digits"))?;
        self.i += 4;
        Ok(n)
    }
}

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
    }

    #[test]
    fn paths() {
        assert_eq!(
            esm_path("@s/p", "^1.0", "/sub"),
            "/@s/p@%5E1.0/sub?target=es2022"
        );
        assert_eq!(
            local("/canvas-confetti@1.9.3?target=es2022").unwrap(),
            "canvas-confetti@1.9.3_target_es2022.js"
        );
        assert_eq!(local("/x@1/es2022/x.mjs").unwrap(), "x@1/es2022/x.mjs");
        assert_eq!(
            local("/@s/core@^1.7.0?target=es2022").unwrap(),
            "@s/core@_1.7.0_target_es2022.js"
        );
        for bad in ["x", "/../x", "/a//b", "/a/./b"] {
            assert!(local(bad).is_err(), "{bad}");
        }
        let src = "/* esm.sh */\nimport \"/a@1/es2022/a.mjs\";\nexport * from \"https://esm.sh/b@2?target=es2022\";\nimport x from \"./c.js\"\nconst d = import(\"/d@1/x.mjs\")";
        assert_eq!(
            imports(src),
            ["/a@1/es2022/a.mjs", "/b@2?target=es2022", "/d@1/x.mjs"]
        );
    }

    #[test]
    fn urls() {
        let deps = vec![("canvas-confetti".to_string(), "1.9.3".to_string())];
        let dev = Npm::new(deps.clone(), false);
        assert_eq!(
            dev.url("canvas-confetti").unwrap(),
            "https://esm.sh/canvas-confetti@1.9.3?target=es2022"
        );
        assert_eq!(dev.used(), ["/canvas-confetti@1.9.3?target=es2022"]);
        let err = dev.url("left-pad").unwrap_err();
        assert!(err.contains("wisp add left-pad"), "{err}");
        let release = Npm::new(deps, true);
        let url = release.url("canvas-confetti/x").unwrap();
        assert!(
            url.starts_with("/_app/c/npm/canvas-confetti@1.9.3/x_target_es2022.js?v="),
            "{url}"
        );
    }

    #[test]
    fn vendored_files_point_at_each_other() {
        let root = std::env::temp_dir().join(format!("wisp-npm-{}", std::process::id()));
        let dir = root.join(".wisp").join("npm");
        std::fs::create_dir_all(dir.join("x@1").join("es2022")).unwrap();
        std::fs::write(
            dir.join("x@1_target_es2022.js"),
            "export * from \"/x@1/es2022/x.mjs\";",
        )
        .unwrap();
        let npm = Npm::new(vec![("x".into(), "1".into())], true);
        let url = npm.url("x").unwrap();
        let err = npm.vendored(&root).unwrap_err();
        assert!(
            err.contains("/x@1/es2022/x.mjs is not in .wisp/npm"),
            "{err}"
        );
        std::fs::write(dir.join("x@1/es2022/x.mjs"), "export default 1;").unwrap();
        let files = npm.vendored(&root).unwrap();
        let _ = std::fs::remove_dir_all(&root);
        let v = url.split_once("?v=").unwrap().1;
        assert_eq!(files.len(), 2);
        assert_eq!(files[0].0, "/_app/c/npm/x@1_target_es2022.js");
        assert_eq!(
            files[0].1,
            format!("export * from \"/_app/c/npm/x@1/es2022/x.mjs?v={v}\";")
        );
        assert_eq!(
            files[1],
            (
                "/_app/c/npm/x@1/es2022/x.mjs".into(),
                "export default 1;".into()
            )
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
        std::fs::write(root.join("package.json"), &added).unwrap();
        let got = deps(&root);
        let _ = std::fs::remove_dir_all(&root);
        assert_eq!(
            got.unwrap(),
            [
                ("@a/b".into(), "2.0.0".into()),
                ("zed".into(), "1.0.0".into())
            ]
        );
        let removed = set_dep(Some(&added), "zed", None).unwrap();
        assert!(!removed.contains("zed") && removed.contains("@a/b"));
        assert!(set_dep(Some(&removed), "zed", None).is_err());
        assert_eq!(
            set_dep(None, "x", Some("1")).unwrap(),
            "{\n  \"dependencies\": {\n    \"x\": \"1\"\n  }\n}\n"
        );
        assert_eq!(
            string_member("{\"name\":\"x\",\"version\":\"1.9.4\"}", "version").as_deref(),
            Some("1.9.4")
        );
        for bad in ["[]", "{", "{\"a\" 1}", "{\"dependencies\": 1}", "{} x"] {
            assert!(set_dep(Some(bad), "x", Some("1")).is_err(), "{bad}");
        }
    }
}
