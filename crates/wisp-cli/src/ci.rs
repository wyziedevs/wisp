//! `wisp deploy init <host>`: a GitHub Actions workflow that builds the app
//! and deploys it to `host` on every push to `main`.

use crate::term;
use std::path::Path;

const FILE: &str = ".github/workflows/deploy.yml";

/// The hosts, each with the Rust target its build needs, the `wisp build`
/// options, the secrets it reads and its deploy steps.
const HOSTS: [(&str, &str, &str, &str, &str); 7] = [
    (
        "cloudflare",
        "wasm32-unknown-unknown",
        "--target cloudflare",
        "CLOUDFLARE_API_TOKEN and CLOUDFLARE_ACCOUNT_ID",
        "      - run: npx --yes wrangler deploy
        working-directory: dist/cloudflare
        env:
          CLOUDFLARE_API_TOKEN: ${{ secrets.CLOUDFLARE_API_TOKEN }}
          CLOUDFLARE_ACCOUNT_ID: ${{ secrets.CLOUDFLARE_ACCOUNT_ID }}
",
    ),
    (
        "deno",
        "wasm32-unknown-unknown",
        "--target deno",
        "none (GitHub's OIDC token)",
        "      - uses: denoland/deployctl@v1
        with:
          project: my-app
          entrypoint: main.ts
          root: dist/deno
",
    ),
    (
        "vercel",
        "wasm32-unknown-unknown",
        "--target vercel",
        "VERCEL_TOKEN, VERCEL_ORG_ID and VERCEL_PROJECT_ID",
        "      - run: npx --yes vercel deploy --prebuilt --prod --token \"$VERCEL_TOKEN\"
        working-directory: dist/vercel
        env:
          VERCEL_TOKEN: ${{ secrets.VERCEL_TOKEN }}
          VERCEL_ORG_ID: ${{ secrets.VERCEL_ORG_ID }}
          VERCEL_PROJECT_ID: ${{ secrets.VERCEL_PROJECT_ID }}
",
    ),
    (
        "netlify",
        "wasm32-unknown-unknown",
        "--target netlify",
        "NETLIFY_AUTH_TOKEN and NETLIFY_SITE_ID",
        "      - run: npx --yes netlify-cli deploy --prod
        working-directory: dist/netlify
        env:
          NETLIFY_AUTH_TOKEN: ${{ secrets.NETLIFY_AUTH_TOKEN }}
          NETLIFY_SITE_ID: ${{ secrets.NETLIFY_SITE_ID }}
",
    ),
    (
        "lambda",
        "x86_64-unknown-linux-musl",
        "--target lambda",
        "AWS_ROLE_ARN (a role GitHub may assume), and the variables AWS_REGION and LAMBDA_FUNCTION",
        "      - uses: aws-actions/configure-aws-credentials@v4
        with:
          role-to-assume: ${{ secrets.AWS_ROLE_ARN }}
          aws-region: ${{ vars.AWS_REGION }}
      - run: aws lambda update-function-code --function-name \"${{ vars.LAMBDA_FUNCTION }}\" --zip-file fileb://dist/lambda/bootstrap.zip
",
    ),
    (
        "fly",
        "",
        "--docker --force",
        "FLY_API_TOKEN",
        "      - uses: superfly/flyctl-actions/setup-flyctl@master
      - run: flyctl deploy --remote-only
        env:
          FLY_API_TOKEN: ${{ secrets.FLY_API_TOKEN }}
",
    ),
    (
        "pages",
        "",
        "--static",
        "none (Settings > Pages > Source: GitHub Actions)",
        "      - uses: actions/upload-pages-artifact@v3
        with:
          path: dist
      - uses: actions/deploy-pages@v4
",
    ),
];

/// `args` after `wisp deploy`: `init <host> [--force]`.
pub fn run(root: &Path, args: &[String]) -> Result<(), String> {
    let names: Vec<&str> = HOSTS.iter().map(|h| h.0).collect();
    let usage = format!(
        "wisp deploy init <host> [--force], for {}.",
        names.join(", ")
    );
    let (host, force) = match args {
        [init, host] if init == "init" => (host, false),
        [init, host, force] | [init, force, host] if init == "init" && force == "--force" => {
            (host, true)
        }
        _ => return Err(format!("Unexpected arguments.\n{usage}")),
    };
    let Some(&(host, target, build, secrets, deploy)) = HOSTS.iter().find(|h| h.0 == host) else {
        return Err(format!("There is no host {host}.\n{usage}"));
    };
    let file = root.join(FILE);
    if file.exists() && !force {
        return Err(format!(
            "{FILE} already exists.\nRun wisp deploy init {host} --force to replace it."
        ));
    }
    let dir = file.parent().unwrap_or(root);
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let package = crate::cargo::package_name(root).unwrap_or_else(|| "my-app".into());
    let deploy = deploy.replace("my-app", &package);
    std::fs::write(&file, workflow(host, target, build, secrets, &deploy))
        .map_err(|e| format!("{FILE}: {e}"))?;
    term::done(&format!("Wrote {FILE}"));
    println!("    It deploys to {host} on every push to main. Secrets it reads: {secrets}.");
    Ok(())
}

fn workflow(host: &str, target: &str, build: &str, secrets: &str, deploy: &str) -> String {
    let permissions = match host {
        "deno" | "lambda" => "permissions:\n  contents: read\n  id-token: write\n",
        "pages" => "permissions:\n  contents: read\n  pages: write\n  id-token: write\n",
        _ => "permissions:\n  contents: read\n",
    };
    let pages = if host == "pages" {
        "    environment: github-pages\n"
    } else {
        ""
    };
    let targets = if target.is_empty() {
        String::new()
    } else {
        format!("        with:\n          targets: {target}\n")
    };
    let install = crate::dep::install().replacen("install", "install --locked", 1);
    format!(
        "# Written by wisp deploy init {host}. Secrets: {secrets}.
name: Deploy
on:
  push:
    branches: [main]
  workflow_dispatch:
{permissions}concurrency: deploy
jobs:
  deploy:
    runs-on: ubuntu-latest
{pages}    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
{targets}      - uses: Swatinem/rust-cache@v2
      - run: {install}
      - run: wisp build {build}
{deploy}"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_a_workflow_per_host_and_keeps_one_there() {
        let dir = std::env::temp_dir().join(format!("wisp-ci-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let args = |s: &str| s.split(' ').map(String::from).collect::<Vec<_>>();
        for (host, ..) in HOSTS {
            run(&dir, &args(&format!("init {host} --force"))).unwrap();
            let yml = std::fs::read_to_string(dir.join(FILE)).unwrap();
            assert!(
                yml.contains("wisp build --") && !yml.contains("\t"),
                "{host}"
            );
            assert!(
                yml.lines().all(|l| l.len() - l.trim_start().len() < 12),
                "{host}"
            );
        }
        let lambda = run(&dir, &args("init lambda"));
        assert!(lambda.unwrap_err().contains("--force"));
        assert!(
            run(&dir, &args("init heroku"))
                .unwrap_err()
                .contains("no host")
        );
        assert!(run(&dir, &args("init")).is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
