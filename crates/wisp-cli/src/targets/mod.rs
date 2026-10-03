//! `wisp build --target <host>`: the app built for WebAssembly
//! (`wasm32-unknown-unknown`, see `wisp::edge`) and a folder that host
//! deploys as it is: `app.wasm`, a small entry shim over the shared
//! `bridge.mjs`, the host's config and the static files for its CDN.

use crate::{cargo, css, deploy, npm, term};
use std::path::Path;
use std::time::Instant;

const BRIDGE: &str = include_str!("bridge.js");
const NODE: &str = include_str!("node.mjs");
const WORKER: &str = include_str!("worker.js");
const DENO: &str = include_str!("deno.ts");
const NETLIFY: &str = include_str!("netlify.mjs");

const WASM_TARGET: &str = "wasm32-unknown-unknown";

/// The hosts `--target` takes besides `static` and `docker`.
pub const HOSTS: [&str; 5] = ["cloudflare", "deno", "vercel", "netlify", "node"];

/// What a host's folder holds: files by path, where `static/` goes, and the
/// command that deploys it.
struct Layout {
    files: Vec<(&'static str, Vec<u8>)>,
    statics: Option<&'static str>,
    deploy: &'static str,
}

pub fn build(root: &Path, host: &str, out: &Path) -> Result<(), String> {
    crate::deploy::check_out(root, out)?;
    let sysroot = std::process::Command::new("rustc")
        .args(["--print", "sysroot"])
        .output()
        .map_err(|e| {
            format!("Could not run rustc: {e}.\nInstall Rust from https://rustup.rs, and open a new terminal.")
        })?;
    let sysroot = String::from_utf8_lossy(&sysroot.stdout).trim().to_string();
    if !Path::new(&sysroot)
        .join("lib/rustlib")
        .join(WASM_TARGET)
        .is_dir()
    {
        return Err(format!(
            "The {host} build needs Rust's WebAssembly target.\nInstall it with rustup target add {WASM_TARGET}, then run this again."
        ));
    }
    let imports = wisp_build::check(root)?;
    css::build(root)?;
    npm::vendor(root, &imports)?;
    let started = Instant::now();
    term::step(&format!("Building for {host} (WebAssembly)"));
    let b = cargo::build_for(root, true, false, Some(WASM_TARGET));
    let wasm = b
        .exe
        .filter(|_| b.ok)
        .ok_or("The build failed.\nThe compiler's errors are above.")?;
    let wasm = std::fs::read(&wasm).map_err(|e| format!("{}: {e}", wasm.display()))?;
    let package = cargo::package_name(root).ok_or("Cargo.toml has no package name.")?;
    let has_static = root.join("static").is_dir();
    let layout = layout(host, &package, wasm, has_static)?;

    let io = |p: &Path, e: std::io::Error| format!("{}: {e}", p.display());
    for (path, bytes) in &layout.files {
        let file = out.join(path);
        std::fs::create_dir_all(file.parent().unwrap_or(out)).map_err(|e| io(&file, e))?;
        std::fs::write(&file, bytes).map_err(|e| io(&file, e))?;
    }
    if let Some(dir) = layout.statics {
        let to = out.join(dir);
        std::fs::create_dir_all(&to).map_err(|e| io(&to, e))?;
        if has_static {
            deploy::copy_dir(&root.join("static"), &to)?;
        }
    }
    term::done(&format!(
        "Wrote {} for {host} {}",
        out.display(),
        term::dim(&format!("in {:.1}s", started.elapsed().as_secs_f64()))
    ));
    println!("    Deploy it from that folder: {}", layout.deploy);
    println!("    Set WISP_SECRET there if the app signs cookies.");
    Ok(())
}

fn layout(host: &str, package: &str, wasm: Vec<u8>, has_static: bool) -> Result<Layout, String> {
    let bridge = || BRIDGE.as_bytes().to_vec();
    let text = |s: &str| s.as_bytes().to_vec();
    Ok(match host {
        "cloudflare" => {
            let assets = if has_static { "\n# Served before the worker runs.\n[assets]\ndirectory = \"public\"\n" } else { "" };
            let wrangler = format!("name = \"{package}\"\nmain = \"worker.js\"\ncompatibility_date = \"2025-09-01\"\n{assets}");
            Layout {
                files: vec![("worker.js", text(WORKER)), ("bridge.mjs", bridge()), ("app.wasm", wasm), ("wrangler.toml", wrangler.into_bytes())],
                statics: has_static.then_some("public"),
                deploy: "npx wrangler deploy",
            }
        }
        "deno" => Layout {
            files: vec![("main.ts", text(DENO)), ("bridge.mjs", bridge()), ("app.wasm", wasm)],
            statics: None,
            deploy: "deployctl deploy --entrypoint main.ts (or deno run -A main.ts to try it)",
        },
        "vercel" => {
            Layout {
                files: vec![
                    (".vercel/output/config.json", text(r#"{"version":3,"routes":[{"handle":"filesystem"},{"src":"/(.*)","dest":"/index"}]}"#)),
                    (
                        ".vercel/output/functions/index.func/.vc-config.json",
                        text(r#"{"runtime":"nodejs22.x","handler":"index.mjs","launcherType":"Nodejs","shouldAddHelpers":false,"supportsResponseStreaming":true}"#),
                    ),
                    (".vercel/output/functions/index.func/index.mjs", text(NODE)),
                    (".vercel/output/functions/index.func/bridge.mjs", bridge()),
                    (".vercel/output/functions/index.func/app.wasm", wasm),
                ],
                statics: Some(".vercel/output/static"),
                deploy: "npx vercel deploy --prebuilt",
            }
        }
        "netlify" => Layout {
            files: vec![
                ("netlify.toml", text("[build]\npublish = \"public\"\n\n[functions]\ndirectory = \"functions\"\n")),
                ("functions/wisp.mjs", text(NETLIFY)),
                ("functions/bridge.mjs", bridge()),
                ("functions/app.wasm.js", format!("export default \"{}\";\n", base64(&wasm)).into_bytes()),
            ],
            statics: Some("public"),
            deploy: "npx netlify deploy --prod",
        },
        "node" => Layout {
            files: vec![
                ("server.mjs", text(NODE)),
                ("bridge.mjs", bridge()),
                ("app.wasm", wasm),
                ("package.json", format!("{{\n  \"name\": \"{package}\",\n  \"private\": true,\n  \"type\": \"module\",\n  \"engines\": {{ \"node\": \">=20\" }},\n  \"scripts\": {{ \"start\": \"node server.mjs\" }}\n}}\n").into_bytes()),
                ("hosts/amplify.md", text(AMPLIFY)),
                ("hosts/firebase.md", text(FIREBASE)),
                ("hosts/azure.md", text(AZURE)),
                ("hosts/stormkit.md", text(STORMKIT)),
                ("hosts/zeabur.md", text(ZEABUR)),
            ],
            statics: None,
            deploy: "npm start runs it; hosts/ says how each host takes it",
        },
        _ => return Err(format!("There is no target {host}.\nThe targets are {}, static and docker.", HOSTS.join(", "))),
    })
}

/// Standard base64 with padding, for the wasm inlined into Netlify's bundle.
fn base64(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    wisp_shared::base64::encode(&mut out, bytes, false);
    out
}

const AMPLIFY: &str = "# AWS Amplify Hosting (compute)

Amplify runs `server.mjs` on port 3000. Put this folder in
`.amplify-hosting/compute/default/`, the app's `static/` in
`.amplify-hosting/static/`, and write `.amplify-hosting/deploy-manifest.json`:

    {\"version\": 1,
     \"routes\": [{\"path\": \"/*\", \"target\": {\"kind\": \"Compute\", \"src\": \"default\"}}],
     \"computeResources\": [{\"name\": \"default\", \"runtime\": \"nodejs20.x\", \"entrypoint\": \"server.mjs\"}]}

Set WISP_SECRET under Environment variables.
";

const FIREBASE: &str = "# Firebase

App Hosting: point a backend at this folder; it runs `npm start`, which
listens on $PORT.

Cloud Functions: in `functions/index.js`, with this folder's files beside it:

    import { onRequest } from 'firebase-functions/v2/https';
    import handler from './server.mjs';
    export const app = onRequest(handler);

and a rewrite of `**` to the function `app` in firebase.json. Set WISP_SECRET
with `firebase functions:secrets:set WISP_SECRET`.
";

const AZURE: &str = "# Azure

App Service or Container Apps (Node 20+): deploy this folder; it runs
`npm start`, which listens on $PORT.

Static Web Apps (managed functions answer under /api only): add an HTTP
function (v4 model) that passes every request on, with the original address
Azure keeps in `x-ms-original-url`, and rewrite everything to it with
`\"navigationFallback\": {\"rewrite\": \"/api/wisp\"}` in staticwebapp.config.json:

    import { app as azure } from '@azure/functions';
    import { app } from './server.mjs';
    azure.http('wisp', { route: '{*path}', authLevel: 'anonymous',
      methods: ['GET', 'HEAD', 'POST', 'PUT', 'PATCH', 'DELETE', 'OPTIONS'],
      handler: async (req) => {
        const url = new URL(req.headers.get('x-ms-original-url') ?? req.url);
        const r = await app.handle({ method: req.method, target: url.pathname + url.search,
          headers: [...req.headers], body: new Uint8Array(await req.arrayBuffer()) });
        return { status: r.status, headers: Object.fromEntries(r.headers), body: r.body };
      } });
";

const STORMKIT: &str = "# Stormkit

Create a Node.js server app from this folder. Start command: `npm start`
(it listens on $PORT). Set WISP_SECRET under Environment variables.
";

const ZEABUR: &str = "# Zeabur

Deploy this folder as a Node.js service; Zeabur runs `npm start`, which
listens on $PORT. Set WISP_SECRET under Variables.
";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_host_has_its_entry_and_the_app() {
        for host in HOSTS {
            let l = layout(host, "site", b"\0asm".to_vec(), true).unwrap();
            let paths: Vec<_> = l.files.iter().map(|(p, _)| *p).collect();
            assert!(paths.iter().any(|p| p.ends_with("bridge.mjs")), "{host}");
            assert!(
                paths
                    .iter()
                    .any(|p| p.ends_with("app.wasm") || p.ends_with("app.wasm.js")),
                "{host}"
            );
        }
        let cf = layout("cloudflare", "site", Vec::new(), true).unwrap();
        let wrangler = &cf
            .files
            .iter()
            .find(|(p, _)| *p == "wrangler.toml")
            .unwrap()
            .1;
        assert!(String::from_utf8_lossy(wrangler).contains("name = \"site\""));
        assert!(
            !String::from_utf8_lossy(
                &layout("cloudflare", "site", Vec::new(), false)
                    .unwrap()
                    .files[3]
                    .1
            )
            .contains("[assets]")
        );
        assert!(layout("heroku", "site", Vec::new(), true).is_err());
    }

    #[test]
    fn shims_import_the_bridge_they_ship_with() {
        for shim in [NODE, WORKER, DENO, NETLIFY] {
            assert!(shim.contains("from './bridge.mjs'"));
        }
        for export in [
            "wisp_buf",
            "wisp_env",
            "wisp_request",
            "wisp_fetched",
            "wisp_current",
            "wisp_poll",
            "wisp_timer",
            "wisp_cancel",
            "wisp_pull",
            "main",
        ] {
            assert!(BRIDGE.contains(&format!("exports.{export}(")), "{export}");
        }
    }
}
