//! `wisp deploy init fly|render|railway`: the host's config file, next to
//! the Dockerfile every one of them builds.

use crate::{cargo, deploy, term};
use std::path::Path;

const HOSTS: [&str; 3] = ["fly", "render", "railway"];

pub fn deploy(root: &Path, args: &[String]) -> Result<(), String> {
    let usage = "Usage: wisp deploy init fly|render|railway [--force]";
    let (host, force) = match args {
        [init, host] if init == "init" => (host.as_str(), false),
        [init, host, f] if init == "init" && f == "--force" => (host.as_str(), true),
        _ => return Err(usage.into()),
    };
    if !HOSTS.contains(&host) {
        return Err(format!("There is no host {host}.\n{usage}"));
    }
    let package = cargo::package_name(root).ok_or("Cargo.toml has no package name.")?;
    init(root, host, &package, force)
}

fn init(root: &Path, host: &str, package: &str, force: bool) -> Result<(), String> {
    let (name, text, next) = config(host, package);
    if root.join(name).exists() && !force {
        return Err(format!(
            "{name} already exists.\nRun wisp deploy init {host} --force to replace it."
        ));
    }
    // The Dockerfile you have is the one it builds.
    if !root.join("Dockerfile").exists() && !root.join(".dockerignore").exists() {
        deploy::docker(root, package, false)?;
    }
    std::fs::write(root.join(name), text).map_err(|e| format!("{name}: {e}"))?;
    term::done(&format!("Wrote {name}"));
    println!("    {next}");
    Ok(())
}

/// The file's name, its text, and what to do next.
fn config(host: &str, package: &str) -> (&'static str, String, &'static str) {
    // Fly and Render name apps in DNS labels: lowercase, digits and dashes.
    let name = package.to_ascii_lowercase().replace('_', "-");
    let package = if matches!(host, "fly" | "render") {
        name.as_str()
    } else {
        package
    };
    match host {
        "fly" => (
            "fly.toml",
            format!(
                "# Written by wisp deploy init fly. The app name must be unique on Fly.
app = \"{package}\"
primary_region = \"iad\"

[build]

[http_service]
  internal_port = 3000
  force_https = true
  auto_stop_machines = \"stop\"
  auto_start_machines = true
  min_machines_running = 0

[[vm]]
  memory = \"256mb\"
  cpu_kind = \"shared\"
  cpus = 1
"
            ),
            "Run fly launch --copy-config --no-deploy, fly secrets set WISP_SECRET=<32 or more random characters>, then fly deploy.",
        ),
        "render" => (
            "render.yaml",
            format!(
                "# Written by wisp deploy init render. Render builds the Dockerfile and sets PORT.
services:
  - type: web
    name: {package}
    runtime: docker
    envVars:
      - key: WISP_SECRET
        generateValue: true
"
            ),
            "Push it, then in Render choose New, Blueprint and pick the repository. WISP_SECRET is made for you.",
        ),
        _ => (
            "railway.toml",
            "# Written by wisp deploy init railway. Railway builds the Dockerfile and sets PORT.
[build]
builder = \"DOCKERFILE\"
dockerfilePath = \"Dockerfile\"

[deploy]
restartPolicyType = \"ON_FAILURE\"
restartPolicyMaxRetries = 10
"
            .into(),
            "Run railway up, and set WISP_SECRET (32 or more random characters) in the service's variables.",
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("wisp-scaffold-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn each_host_gets_its_file_and_the_dockerfile() {
        for (host, file, has) in [
            ("fly", "fly.toml", "app = \"my-app\""),
            ("render", "render.yaml", "name: my-app"),
            ("railway", "railway.toml", "builder = \"DOCKERFILE\""),
        ] {
            let dir = temp(host);
            init(&dir, host, "my-app", false).unwrap();
            let text = std::fs::read_to_string(dir.join(file)).unwrap();
            assert!(text.contains(has), "{text}");
            let docker = std::fs::read_to_string(dir.join("Dockerfile")).unwrap();
            assert!(docker.contains("cp target/release/my-app /server"));
            let _ = std::fs::remove_dir_all(&dir);
        }
    }

    #[test]
    fn hosts_get_a_dns_safe_app_name() {
        let (_, fly, _) = config("fly", "My_App");
        assert!(fly.contains("app = \"my-app\""), "{fly}");
        let (_, render, _) = config("render", "My_App");
        assert!(render.contains("name: my-app"), "{render}");
    }

    #[test]
    fn it_keeps_what_exists() {
        let dir = temp("keep");
        std::fs::write(dir.join("Dockerfile"), "mine").unwrap();
        init(&dir, "fly", "my-app", false).unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.join("Dockerfile")).unwrap(),
            "mine"
        );
        let again = init(&dir, "fly", "my-app", false).unwrap_err();
        assert!(again.starts_with("fly.toml already exists"), "{again}");
        std::fs::write(dir.join("fly.toml"), "mine").unwrap();
        init(&dir, "fly", "my-app", true).unwrap();
        let text = std::fs::read_to_string(dir.join("fly.toml")).unwrap();
        assert!(text.contains("internal_port = 3000"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
