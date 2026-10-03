//! What an editor asks of the compiler (`wisp lsp`): one file's problem as
//! it is typed, from the buffer, and the whole project's from disk. Both are
//! the build's own checks; nothing here can panic on a bad file.

pub use crate::codegen::Comp as Component;
pub use crate::template::PropDecl;

use crate::routes::Seg;

use std::path::Path;

/// A problem at a place in a file: 1-based `line`, 1-based `col` in
/// characters (0 when the compiler names the line alone, or the file).
#[derive(Clone, Debug, PartialEq)]
pub struct Diag {
    pub line: u32,
    pub col: u32,
    pub msg: String,
}

/// The components of the app at `root`, as the build reads them from disk.
pub fn components(root: &Path) -> Result<Vec<Component>, String> {
    crate::codegen::components(root)
}

/// The first problem of the `.wisp` file `rel` (from the project root,
/// `/`-separated) whose text is `src`, by the checks that need no other file
/// but the app's components (`None`: they could not be read, so component
/// uses go unchecked).
pub fn check_file(rel: &str, src: &str, comps: Option<&[Component]>) -> Option<Diag> {
    let src = src.strip_prefix('\u{feff}').unwrap_or(src);
    let src = src.replace("\r\n", "\n");
    let fail = |e: String| Some(locate(&e));
    let (rust, markup) = match crate::split_front(&src) {
        Ok(x) => x,
        Err(e) => return fail(e),
    };
    let block = rust.clone();
    let drawn = rel.ends_with("+page.wisp")
        && (rust.as_deref())
            .and_then(|r| crate::rust_scan::scan(&crate::rust_scan::split_items(r).0).ok())
            .is_some_and(|i| i.drawn());
    let (t, _) = match crate::parse_markup(&markup, rust, &[], rel, drawn) {
        Ok(x) => x,
        Err(e) => return fail(e),
    };
    let component = rel.starts_with("src/components/");
    if let (false, Some((_, line))) = (component, &t.props) {
        return fail(format!(
            "{line}: only components, in src/components, take props"
        ));
    }
    if let Some(code) = block {
        if component {
            return fail("1: a component takes what it shows as {@props …}; a `---` block of Rust is for pages and layouts".into());
        }
        let (items, _) = crate::rust_scan::split_items(&code);
        if let Err(e) = crate::rust_scan::scan(&items).and_then(|i| i.check()) {
            return fail(e);
        }
    }
    let comps = comps?;
    crate::codegen::check_components(&t.nodes, &t, comps, "", false)
        .err()
        .map(|e| locate(e.trim_start_matches(':')))
}

/// The first problem of the project at `root`, as `wisp check` finds it:
/// the file it is in (from the root, if it names one) and where.
pub fn check_project(root: &Path) -> Option<(Option<String>, Diag)> {
    let e = crate::check(root).err()?;
    let (file, rest) = match e.split_once(':') {
        Some((f, rest)) if root.join(f).is_file() => (Some(f.to_string()), rest.trim_start()),
        _ => (None, e.as_str()),
    };
    Some((file, locate(rest)))
}

/// The route params a file under `src/routes` (`rel`) reads as locals,
/// with their Rust types: `[id=int]` is `id: u64`.
pub fn route_params(rel: &str) -> Vec<(String, &'static str)> {
    let dirs = rel.strip_prefix("src/routes/").unwrap_or("").split('/');
    let segs = dirs.filter_map(|d| crate::routes::parse_segment(d).ok().flatten());
    segs.filter_map(|s| {
        let int = |m: &Option<String>| m.as_deref() == Some("int");
        let ty = match &s {
            Seg::Static(_) => return None,
            Seg::Param(_, m) if int(m) => "u64",
            Seg::Optional(_, m) if int(m) => "Option<u64>",
            Seg::Optional(..) => "Option<String>",
            Seg::Param(..) | Seg::Rest(_) => "String",
        };
        Some((s.param()?.to_string(), ty))
    })
    .collect()
}

/// `line:col: msg`, `line: msg` or `msg`, as the compiler words a problem.
fn locate(e: &str) -> Diag {
    let mut d = Diag {
        line: 0,
        col: 0,
        msg: e.trim().to_string(),
    };
    let mut rest = e;
    for slot in [&mut d.line, &mut d.col] {
        let Some((n, after)) = rest.split_once(':') else {
            break;
        };
        let Ok(n) = n.trim().parse() else { break };
        *slot = n;
        rest = after;
        d.msg = after.trim().to_string();
    }
    d
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locates() {
        let d = |line, col, msg: &str| Diag {
            line,
            col,
            msg: msg.into(),
        };
        assert_eq!(locate("3:4: bad"), d(3, 4, "bad"));
        assert_eq!(locate("3: bad: x"), d(3, 0, "bad: x"));
        assert_eq!(locate("bad"), d(0, 0, "bad"));
    }

    #[test]
    fn checks_a_buffer() {
        let page = "src/routes/+page.wisp";
        assert_eq!(check_file(page, "<p>{x}</p>", None), None);
        let e = check_file(page, "<p>\n{#if x}</p>", None).unwrap();
        assert_eq!(e.line, 2, "{e:?}");
        let e = check_file(page, "---\nlet x = 1\n<p>", None).unwrap();
        assert!(e.msg.contains("`---`"), "{e:?}");
        let e = check_file(page, "<Card />", Some(&[])).unwrap();
        assert!(e.msg.contains("no component `Card`"), "{e:?}");
        assert_eq!(e.line, 1);
    }

    #[test]
    fn params() {
        let p = route_params("src/routes/(app)/[[lang]]/u/[id=int]/[...rest]/+page.wisp");
        let want = [
            ("lang", "Option<String>"),
            ("id", "u64"),
            ("rest", "String"),
        ];
        assert_eq!(p, want.map(|(n, t)| (n.to_string(), t)));
    }
}
