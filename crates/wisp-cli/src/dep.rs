//! Where apps get Wisp from: the one switch for the dependency lines `wisp
//! new` writes, the CI workflow's install step and the update hint. Today
//! the git repository; once the crates are on crates.io, `WISP_DEP` becomes
//! `Dep::Crates("0.1")` (RELEASING.md) and nothing else changes.

/// A source of the Wisp crates.
pub enum Dep {
    /// The repository, by git URL.
    Git(&'static str),
    /// crates.io, by version requirement.
    #[allow(dead_code, reason = "the switch's other side, until the first publish")]
    Crates(&'static str),
}

/// The switch.
pub const WISP_DEP: Dep = Dep::Git("https://wisp.ar0.eu");

/// The published package of each crate an app names, by its key in the
/// app's Cargo.toml (`wisp`, `wisp-build`), so `use wisp::..` never changes.
pub fn package(key: &str) -> &str {
    match key {
        "wisp" => "wisp-web-rt",
        "wisp-build" => "wisp-web-build",
        other => other,
    }
}

/// The app's dependency line for `key` from `WISP_DEP`.
pub fn line(key: &str) -> String {
    let pkg = package(key);
    match WISP_DEP {
        Dep::Git(url) => format!("{key} = {{ git = \"{url}\", package = \"{pkg}\" }}"),
        Dep::Crates(v) => format!("{key} = {{ version = \"{v}\", package = \"{pkg}\" }}"),
    }
}

/// The app's dependency line for `key` from a Wisp checkout at `repo`.
pub fn path_line(key: &str, repo: &str) -> String {
    let pkg = package(key);
    format!("{key} = {{ path = \"{repo}/crates/{key}\", package = \"{pkg}\" }}")
}

/// The command that installs this CLI, without flags. Always the crates.io
/// name; it works once the crates are published (RELEASING.md).
pub fn install() -> String {
    "cargo install wisp-web".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_follow_the_switch() {
        let (wisp, build, cmd) = (line("wisp"), line("wisp-build"), install());
        match WISP_DEP {
            Dep::Git(url) => {
                assert_eq!(
                    wisp,
                    format!("wisp = {{ git = \"{url}\", package = \"wisp-web-rt\" }}")
                );
                assert!(build.contains(url) && build.contains("package = \"wisp-web-build\""));
            }
            Dep::Crates(v) => {
                assert_eq!(
                    wisp,
                    format!("wisp = {{ version = \"{v}\", package = \"wisp-web-rt\" }}")
                );
                assert!(build.contains("package = \"wisp-web-build\""));
            }
        }
        assert_eq!(cmd, "cargo install wisp-web");
        assert_eq!(
            path_line("wisp", "/w"),
            "wisp = { path = \"/w/crates/wisp\", package = \"wisp-web-rt\" }"
        );
    }
}
