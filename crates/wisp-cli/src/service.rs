//! `wisp service install|uninstall|start|stop|status`: run the app's release
//! binary as an OS service. Opt-in; nothing here touches the running server.
//!
//! Linux writes a systemd unit, macOS a launchd daemon. Windows uses a
//! scheduled task that starts at boot: a real Windows service must answer the
//! Service Control Manager through `StartServiceCtrlDispatcher`, which needs
//! `unsafe` FFI (or a dependency that has it), and Wisp has none. The task
//! runs the same binary; `stop` ends the process without a graceful drain.
//! systemd and launchd stop it with SIGTERM, which the runtime drains on.

use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Clone, Copy, PartialEq, Debug)]
enum Os {
    Linux,
    Mac,
    Windows,
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Action {
    Install,
    Uninstall,
    Start,
    Stop,
    Status,
}

#[derive(Debug, Default, PartialEq)]
struct Options {
    name: Option<String>,
    user: Option<String>,
    port: Option<u16>,
    dry_run: bool,
}

/// A command to run; `lenient` means a failure does not stop the rest.
#[derive(Debug, PartialEq)]
struct Cmd {
    args: Vec<String>,
    lenient: bool,
}

/// What to write and run for one action.
#[derive(Debug, Default, PartialEq)]
struct Plan {
    /// Files to write: path, text, and whether only the owner may read it.
    files: Vec<(String, String, bool)>,
    /// Files that are already there stay as they are (the env file).
    keep: Vec<String>,
    cmds: Vec<Cmd>,
}

const USAGE: &str = "wisp service takes install, uninstall, start, stop or status, and for install --user <name>, --port <n>, --name <service>, --dry-run.";

pub fn run(root: &Path, args: &[String]) -> Result<(), String> {
    let (action, o) = parse(args)?;
    let os = if cfg!(windows) {
        Os::Windows
    } else if cfg!(target_os = "macos") {
        Os::Mac
    } else {
        Os::Linux
    };
    let package = crate::cargo::package_name(root).ok_or("Cargo.toml has no package name.")?;
    let name = o.name.clone().unwrap_or_else(|| package.clone());
    check_name(&name)?;
    let dir =
        std::fs::canonicalize(root).map_err(|e| format!("Cannot read the app folder: {e}."))?;
    let dir = clean(&dir);
    check_path(&dir)?;
    let exe = dir.join("target/release").join(exe_name(&package));
    if action == Action::Install && !o.dry_run && !exe.exists() {
        return Err(format!(
            "{} is not built.\nRun wisp build, then wisp service install.",
            exe.display()
        ));
    }
    let plan = plan(os, action, &name, &dir, &exe, &o);
    if o.dry_run {
        for (path, text, _) in &plan.files {
            println!("# {path}\n{text}");
        }
        for c in &plan.cmds {
            println!("{}", c.args.join(" "));
        }
        return Ok(());
    }
    for (path, text, private) in &plan.files {
        if plan.keep.contains(path) && Path::new(path).exists() {
            continue;
        }
        write(path, text, *private)?;
    }
    for c in &plan.cmds {
        let status = Command::new(&c.args[0]).args(&c.args[1..]).status();
        let ok = status.as_ref().is_ok_and(|s| s.success());
        if !ok && !c.lenient && action != Action::Status {
            let why = status.map_or_else(|e| e.to_string(), |s| s.to_string());
            return Err(format!(
                "{} failed: {why}.\nRun it as an administrator (sudo on Linux and macOS).",
                c.args.join(" ")
            ));
        }
    }
    Ok(())
}

fn parse(args: &[String]) -> Result<(Action, Options), String> {
    let action = match args.first().map(String::as_str) {
        Some("install") => Action::Install,
        Some("uninstall") => Action::Uninstall,
        Some("start") => Action::Start,
        Some("stop") => Action::Stop,
        Some("status") => Action::Status,
        _ => return Err(USAGE.into()),
    };
    let mut o = Options::default();
    let mut it = args[1..].iter();
    while let Some(arg) = it.next() {
        let mut value = || {
            it.next()
                .cloned()
                .ok_or_else(|| format!("{arg} needs a value.\n{USAGE}"))
        };
        match arg.as_str() {
            "--dry-run" => o.dry_run = true,
            "--user" => o.user = Some(value()?),
            "--name" => o.name = Some(value()?),
            "--port" => {
                let v = value()?;
                o.port = Some(
                    v.parse()
                        .map_err(|_| format!("{v} is not a port number."))?,
                );
            }
            _ => return Err(format!("There is no option {arg}.\n{USAGE}")),
        }
    }
    for s in [&o.name, &o.user].into_iter().flatten() {
        check_name(s)?;
    }
    Ok((action, o))
}

/// Names go into file paths and command lines: plain characters only.
fn check_name(s: &str) -> Result<(), String> {
    let ok = !s.is_empty()
        && s.starts_with(|c: char| c.is_ascii_alphanumeric() || c == '_')
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'));
    match ok {
        true => Ok(()),
        false => Err(format!(
            "{s} is not a service or user name.\nUse letters, digits, - _ and ., starting with a letter or digit."
        )),
    }
}

/// The app folder goes into unit files and a `cmd` line: no control
/// characters, quotes or `%`, which no quoting there carries safely.
fn check_path(p: &Path) -> Result<(), String> {
    let s = p.to_string_lossy();
    match s.chars().any(|c| c.is_control() || matches!(c, '"' | '%')) {
        true => Err(format!(
            "{s} has a quote, % or control character in it.\nMove the app to a plainer folder."
        )),
        false => Ok(()),
    }
}

fn exe_name(package: &str) -> String {
    match cfg!(windows) {
        true => format!("{package}.exe"),
        false => package.to_string(),
    }
}

/// A path without the `\\?\` prefix `canonicalize` gives on Windows.
fn clean(p: &Path) -> PathBuf {
    let s = p.to_string_lossy();
    PathBuf::from(s.strip_prefix(r"\\?\").unwrap_or(&s).to_string())
}

fn write(path: &str, text: &str, private: bool) -> Result<(), String> {
    let fail = |e: std::io::Error| {
        format!("Cannot write {path}: {e}.\nRun it as an administrator (sudo on Linux and macOS).")
    };
    std::fs::write(path, text).map_err(fail)?;
    #[cfg(unix)]
    if private {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).map_err(fail)?;
    }
    let _ = private;
    Ok(())
}

fn cmd(args: &[&str]) -> Cmd {
    Cmd {
        args: args.iter().map(|s| s.to_string()).collect(),
        lenient: false,
    }
}

fn lenient(args: &[&str]) -> Cmd {
    Cmd {
        lenient: true,
        ..cmd(args)
    }
}

fn plan(os: Os, action: Action, name: &str, dir: &Path, exe: &Path, o: &Options) -> Plan {
    match os {
        Os::Linux => linux(action, name, dir, exe, o),
        Os::Mac => mac(action, name, dir, exe, o),
        Os::Windows => windows(action, name, dir, exe, o),
    }
}

/// The systemd unit: restarts on failure, reads `/etc/<name>.env`, raises the
/// file limit, and may bind a port below 1024 without root.
fn unit(name: &str, dir: &Path, exe: &Path, o: &Options) -> String {
    let mut s = format!(
        "[Unit]\nDescription={name} (Wisp app)\nAfter=network-online.target\nWants=network-online.target\n\n[Service]\nType=simple\nWorkingDirectory={}\nExecStart={}\nEnvironmentFile=-/etc/{name}.env\n",
        unit_text(dir, false),
        unit_text(exe, true)
    );
    if let Some(port) = o.port {
        s += &format!("Environment=PORT={port}\n");
    }
    if let Some(user) = &o.user {
        s += &format!("User={user}\n");
    }
    if o.port.is_some_and(|p| p < 1024) {
        s += "AmbientCapabilities=CAP_NET_BIND_SERVICE\nCapabilityBoundingSet=CAP_NET_BIND_SERVICE\n";
    }
    s += "Restart=on-failure\nRestartSec=2\nLimitNOFILE=1048576\nTimeoutStopSec=15\nKillSignal=SIGTERM\n\n[Install]\nWantedBy=multi-user.target\n";
    s
}

/// A path as a unit file reads it: `%` doubled (specifiers), and in a
/// command line `$` too (variables) and quoted when a space or quote would
/// split it.
fn unit_text(p: &Path, command: bool) -> String {
    let s = p.to_string_lossy().replace('%', "%%");
    if !command {
        return s;
    }
    let s = s.replace('$', "$$");
    match s.contains([' ', '\t', '"', '\'', '\\', ';']) {
        true => format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\"")),
        false => s,
    }
}

/// `s` as XML text.
fn xml(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn linux(action: Action, name: &str, dir: &Path, exe: &Path, o: &Options) -> Plan {
    let unit_path = format!("/etc/systemd/system/{name}.service");
    let svc = format!("{name}.service");
    let mut p = Plan::default();
    match action {
        Action::Install => {
            let env = format!("/etc/{name}.env");
            let text = "# Settings for the app, one KEY=value per line.\n# WISP_SECRET=at-least-32-random-characters\n";
            p.files.push((unit_path, unit(name, dir, exe, o), false));
            p.files.push((env.clone(), text.into(), true));
            p.keep.push(env);
            p.cmds = vec![
                cmd(&["systemctl", "daemon-reload"]),
                cmd(&["systemctl", "enable", &svc]),
                cmd(&["systemctl", "start", &svc]),
            ];
        }
        Action::Uninstall => {
            p.cmds = vec![
                lenient(&["systemctl", "stop", &svc]),
                lenient(&["systemctl", "disable", &svc]),
                lenient(&["rm", "-f", &unit_path]),
                cmd(&["systemctl", "daemon-reload"]),
            ];
        }
        Action::Start => p.cmds = vec![cmd(&["systemctl", "start", &svc])],
        Action::Stop => p.cmds = vec![cmd(&["systemctl", "stop", &svc])],
        Action::Status => p.cmds = vec![cmd(&["systemctl", "status", "--no-pager", &svc])],
    }
    p
}

fn mac(action: Action, name: &str, dir: &Path, exe: &Path, o: &Options) -> Plan {
    let label = format!("wisp.{name}");
    let path = format!("/Library/LaunchDaemons/{label}.plist");
    let target = format!("system/{label}");
    let mut p = Plan::default();
    match action {
        Action::Install => {
            let env = o.port.map_or(String::new(), |port| {
                format!("  <key>EnvironmentVariables</key>\n  <dict><key>PORT</key><string>{port}</string></dict>\n")
            });
            let user = o.user.as_ref().map_or(String::new(), |u| {
                format!("  <key>UserName</key>\n  <string>{u}</string>\n")
            });
            let text = format!(
                "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<plist version=\"1.0\">\n<dict>\n  <key>Label</key>\n  <string>{label}</string>\n  <key>ProgramArguments</key>\n  <array><string>{}</string></array>\n  <key>WorkingDirectory</key>\n  <string>{}</string>\n{env}{user}  <key>RunAtLoad</key>\n  <true/>\n  <key>KeepAlive</key>\n  <dict><key>SuccessfulExit</key><false/></dict>\n  <key>SoftResourceLimits</key>\n  <dict><key>NumberOfFiles</key><integer>65536</integer></dict>\n</dict>\n</plist>\n",
                xml(&exe.to_string_lossy()),
                xml(&dir.to_string_lossy())
            );
            p.files.push((path.clone(), text, false));
            p.cmds = vec![cmd(&["launchctl", "bootstrap", "system", &path])];
        }
        Action::Uninstall => {
            p.cmds = vec![
                lenient(&["launchctl", "bootout", &target]),
                lenient(&["rm", "-f", &path]),
            ];
        }
        Action::Start => p.cmds = vec![cmd(&["launchctl", "kickstart", &target])],
        Action::Stop => p.cmds = vec![cmd(&["launchctl", "kill", "SIGTERM", &target])],
        Action::Status => p.cmds = vec![cmd(&["launchctl", "print", &target])],
    }
    p
}

fn windows(action: Action, name: &str, dir: &Path, exe: &Path, o: &Options) -> Plan {
    let mut p = Plan::default();
    match action {
        Action::Install => {
            let port = o.port.map_or(String::new(), |p| format!("set PORT={p}&& "));
            let run = format!(
                "cmd /c cd /d \"{}\" && {port}\"{}\"",
                dir.display(),
                exe.display()
            );
            let user = o.user.as_deref().unwrap_or("SYSTEM");
            p.cmds = vec![
                cmd(&[
                    "schtasks", "/Create", "/TN", name, "/TR", &run, "/SC", "ONSTART", "/RU", user,
                    "/RL", "HIGHEST", "/F",
                ]),
                cmd(&["schtasks", "/Run", "/TN", name]),
            ];
        }
        Action::Uninstall => {
            p.cmds = vec![
                lenient(&["schtasks", "/End", "/TN", name]),
                cmd(&["schtasks", "/Delete", "/TN", name, "/F"]),
            ];
        }
        Action::Start => p.cmds = vec![cmd(&["schtasks", "/Run", "/TN", name])],
        Action::Stop => p.cmds = vec![cmd(&["schtasks", "/End", "/TN", name])],
        Action::Status => {
            p.cmds = vec![cmd(&[
                "schtasks", "/Query", "/TN", name, "/V", "/FO", "LIST",
            ])]
        }
    }
    p
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opts(port: Option<u16>, user: Option<&str>) -> Options {
        Options {
            port,
            user: user.map(String::from),
            ..Options::default()
        }
    }

    fn args(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn odd_paths() {
        let d = Path::new("/srv/my app & co");
        let e = Path::new("/srv/my app & co/target/release/x");
        let o = Options::default();
        let u = unit("x", d, e, &o);
        assert!(
            u.contains("ExecStart=\"/srv/my app & co/target/release/x\"\n"),
            "{u}"
        );
        let p = plan(Os::Mac, Action::Install, "x", d, e, &o);
        assert!(
            p.files[0]
                .1
                .contains("<string>/srv/my app &amp; co</string>")
        );
        for bad in ["/a\nExecStartPre=/bin/sh", "/a%h", "/a\"b"] {
            assert!(check_path(Path::new(bad)).is_err(), "{bad}");
        }
        assert!(check_path(d).is_ok());
    }

    #[test]
    fn systemd_unit() {
        let (d, e) = (
            Path::new("/srv/blog"),
            Path::new("/srv/blog/target/release/blog"),
        );
        let u = unit("blog", d, e, &opts(Some(80), Some("www")));
        for want in [
            "WorkingDirectory=/srv/blog\n",
            "ExecStart=/srv/blog/target/release/blog\n",
            "EnvironmentFile=-/etc/blog.env\n",
            "Environment=PORT=80\n",
            "User=www\n",
            "AmbientCapabilities=CAP_NET_BIND_SERVICE\n",
            "Restart=on-failure\n",
            "LimitNOFILE=1048576\n",
            "WantedBy=multi-user.target\n",
        ] {
            assert!(u.contains(want), "{want}");
        }
        let u = unit("blog", d, e, &opts(Some(8080), None));
        assert!(!u.contains("AmbientCapabilities") && !u.contains("User="));
    }

    #[test]
    fn paths_with_spaces_and_symbols_are_quoted_for_the_unit_and_plist() {
        let (d, e) = (
            Path::new("/srv/my app"),
            Path::new("/srv/my app/target/release/my-app"),
        );
        let u = unit("x", d, e, &Options::default());
        assert!(u.contains("WorkingDirectory=/srv/my app\n"), "{u}");
        let quoted = "ExecStart=\"/srv/my app/target/release/my-app\"\n";
        assert!(u.contains(quoted), "{u}");
        let (d, e) = (Path::new("/a%b"), Path::new("/a%b/$x"));
        let u = unit("x", d, e, &Options::default());
        assert!(u.contains("WorkingDirectory=/a%%b\n") && u.contains("ExecStart=/a%%b/$$x\n"));
        let (d, e) = (Path::new("/a&b"), Path::new("/a&b/x"));
        let p = plan(Os::Mac, Action::Install, "x", d, e, &Options::default());
        assert!(p.files[0].1.contains("<string>/a&amp;b/x</string>"));
    }

    #[test]
    fn linux_commands() {
        let (d, e) = (Path::new("/a"), Path::new("/a/x"));
        let p = plan(Os::Linux, Action::Install, "x", d, e, &Options::default());
        let cmds: Vec<String> = p.cmds.iter().map(|c| c.args.join(" ")).collect();
        assert_eq!(
            cmds,
            [
                "systemctl daemon-reload",
                "systemctl enable x.service",
                "systemctl start x.service"
            ]
        );
        assert_eq!(p.files[0].0, "/etc/systemd/system/x.service");
        assert!(p.files[1].2 && p.keep == ["/etc/x.env"]);
        let p = plan(Os::Linux, Action::Uninstall, "x", d, e, &Options::default());
        assert!(p.cmds[0].lenient && p.cmds[0].args[1] == "stop");
    }

    #[test]
    fn windows_task() {
        let (d, e) = (
            Path::new(r"C:\app"),
            Path::new(r"C:\app\target\release\x.exe"),
        );
        let p = plan(
            Os::Windows,
            Action::Install,
            "x",
            d,
            e,
            &opts(Some(80), None),
        );
        let c = &p.cmds[0].args;
        assert_eq!(&c[..2], ["schtasks", "/Create"]);
        assert!(c.contains(&"ONSTART".to_string()) && c.contains(&"SYSTEM".to_string()));
        assert!(
            c.iter()
                .any(|a| a.contains("set PORT=80&&") && a.contains("x.exe"))
        );
    }

    #[test]
    fn mac_plist() {
        let (d, e) = (Path::new("/a"), Path::new("/a/x"));
        let p = plan(Os::Mac, Action::Install, "x", d, e, &opts(None, Some("me")));
        assert_eq!(p.files[0].0, "/Library/LaunchDaemons/wisp.x.plist");
        let t = &p.files[0].1;
        assert!(t.contains("<string>me</string>") && t.contains("<string>/a/x</string>"));
    }

    #[test]
    fn parses() {
        let (a, o) = parse(&args(&[
            "install",
            "--port",
            "80",
            "--user",
            "u",
            "--dry-run",
        ]))
        .unwrap();
        assert_eq!(a, Action::Install);
        let want = Options {
            name: None,
            user: Some("u".into()),
            port: Some(80),
            dry_run: true,
        };
        assert_eq!(o, want);
        assert!(parse(&args(&["install", "--bogus"])).is_err());
        assert!(parse(&args(&[])).is_err());
        assert!(check_name("a b").is_err() && check_name("my-app").is_ok());
        assert!(check_name("--now").is_err() && check_name("..").is_err());
    }
}
