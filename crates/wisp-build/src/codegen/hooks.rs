//! `src/hooks.rs`: `init` and `before`.

use super::*;

/// `src/hooks.rs`, if there is one, checked, as a module with shims for
/// the hooks it has: `init` and `before`, which look the way they are
/// called, and no other public function (a typo would never run). With it,
/// whether `before` can make a request wait (`init` runs before any).
pub(super) fn hooks(root: &Path) -> Result<(Option<UserMod>, bool), String> {
    // A `mod hooks;` of the app's own would compile the file a second time,
    // with statics of its own.
    let main = root.join("src").join("main.rs");
    if let Ok(src) = crate::read_source(&main) {
        for (n, line) in src.lines().enumerate() {
            let t = line.trim_start().trim_start_matches("pub ").trim_start();
            if t.starts_with("mod hooks;") || t.starts_with("mod hooks ") {
                return Err(format!(
                    "src/main.rs:{}: remove `mod hooks`: Wisp includes src/hooks.rs itself",
                    n + 1
                ));
            }
        }
    }
    let file = root.join("src").join("hooks.rs");
    if !file.exists() {
        return Ok((None, false));
    }
    let src = crate::read_source(&file).map_err(|e| format!("src/hooks.rs: {e}"))?;
    let items = rust_scan::scan(&src).map_err(|e| format!("src/hooks.rs:{e}"))?;
    items
        .check_inner()
        .map_err(|e| format!("src/hooks.rs:{e}"))?;
    let mut shims = Vec::new();
    for f in &items.fns {
        let at = |msg: String| format!("src/hooks.rs:{}: {msg}", f.line);
        if f.action {
            return Err(at(format!(
                "`{}` is marked #[action], but actions belong in a +page.rs",
                f.name
            )));
        }
        if f.remote.is_some() {
            return Err(at(format!(
                "`{}` is marked #[remote], which belongs in a page or src/remote.rs",
                f.name
            )));
        }
        match f.name.as_str() {
            "init" => {
                if !f.params.is_empty() {
                    return Err(at("`init` runs once, before the server takes requests, so it has no `cx`: `async fn init()`".into()));
                }
                if f.returns_kind() != Returns::Nothing {
                    return Err(at(format!(
                        "`init` returns `{}`; it returns nothing, or `Result<()>` so it can use `?`",
                        f.returns
                    )));
                }
                shims.push(shim(f, Shim::Init).map_err(|e| format!("src/hooks.rs:{e}"))?);
            }
            "before" => {
                check_before(f).map_err(at)?;
                shims.push(shim(f, Shim::Answer).map_err(|e| format!("src/hooks.rs:{e}"))?);
            }
            // Plain functions, which the server calls itself when they are there.
            "reroute" => {
                if f.params.len() != 1 || f.is_async || !f.params[0].1.contains("str") {
                    return Err(at("`reroute` is `fn reroute(path: &str) -> &str`: sync, it runs on every request's path before the route is looked for, and returns a part of `path` (or a path with no parameters)".into()));
                }
                shims.push("pub fn reroute(path: &str) -> &str { super::reroute(path) }".into());
            }
            "after" | "report" => {
                let after = f.name == "after";
                let ty = if after { "Reply" } else { "Error" };
                if f.params.len() != 2
                    || !rust_scan::is_cx(&f.params[0].1)
                    || !f.params[1].1.contains(ty)
                    || f.is_async
                {
                    return Err(at(match after {
                        true => "`after` is `fn after(cx: &mut Cx, reply: &mut Reply)`: sync, and runs on every reply".into(),
                        false => "`report` is `fn report(cx: &mut Cx, err: &Error)`: sync, and runs on every 5xx".into(),
                    }));
                }
                let (sig, call) = match after {
                    true => ("reply: &mut ::wisp::Reply", "cx, reply"),
                    false => ("err: &::wisp::Error", "cx, err"),
                };
                let name = &f.name;
                shims.push(format!(
                    "pub fn {name}(cx: &mut ::wisp::Cx, {sig}) {{ super::{name}({call}) }}"
                ));
            }
            name if f.public => {
                return Err(at(format!(
                    "`{name}` is not a hook: src/hooks.rs has `init`, `before`, `after`, `report` and `reroute`. Make it private if it is a helper."
                )));
            }
            _ => {}
        }
    }
    let guard = guards(None, &items, "src/hooks.rs", &mut shims)?;
    add_guard(&mut shims, &guard, items.function("before").is_some());
    let waits = items.function("before").is_some_and(|f| f.is_async);
    Ok((
        Some(UserMod::new("hooks".into(), file, None, shims, &items)),
        waits,
    ))
}
