//! `wisp build --target <host>`: the app built for WebAssembly
//! (`wasm32-unknown-unknown`, see `wisp::edge`) and a folder that host
//! deploys as it is: `app.wasm`, a small entry shim over the shared
//! `bridge.mjs`, the host's config and the static files for its CDN. For
//! `lambda`, the app's own binary for Linux, zipped as Lambda's `bootstrap`.

use crate::{cargo, css, deploy, images, npm, term};
use std::path::Path;
use std::time::Instant;

const BRIDGE: &str = include_str!("bridge.js");
const NODE: &str = include_str!("node.mjs");
const WORKER: &str = include_str!("worker.js");
const DENO: &str = include_str!("deno.ts");
const NETLIFY: &str = include_str!("netlify.mjs");
const BUN: &str = include_str!("bun.mjs");

const WASM_TARGET: &str = "wasm32-unknown-unknown";
/// Static, so it runs on any Linux: `provided.al2023` and older.
const LAMBDA_TARGET: &str = "x86_64-unknown-linux-musl";

/// The hosts `--target` takes besides `static`, `docker` and `native`.
pub const HOSTS: [&str; 8] = [
    "cloudflare",
    "pages",
    "deno",
    "vercel",
    "netlify",
    "node",
    "bun",
    "lambda",
];

/// A variable each host's build sets, and the target a plain `wisp build`
/// picks there. Lambda's builds run anywhere, so it is not among them.
const CI: [(&str, &str); 6] = [
    ("WORKERS_CI", "cloudflare"),
    ("CF_PAGES", "cloudflare"),
    ("VERCEL", "vercel"),
    ("NETLIFY", "netlify"),
    ("DENO_DEPLOYMENT_ID", "deno"),
    ("AWS_APP_ID", "node"),
];

/// The host whose build this is, as the variable that says so and the
/// target: `var` reads the environment.
pub fn detect(var: impl Fn(&str) -> Option<String>) -> Option<(&'static str, &'static str)> {
    CI.into_iter()
        .find(|(name, _)| var(name).is_some_and(|v| !v.is_empty() && v != "0" && v != "false"))
}

/// What a host's folder holds: files by path, where `static/` goes, and the
/// command that deploys it.
struct Layout {
    files: Vec<(&'static str, Vec<u8>)>,
    statics: Option<&'static str>,
    deploy: &'static str,
}

pub fn build(root: &Path, host: &str, out: &Path) -> Result<(), String> {
    // Vercel's folder is `.vercel/output`, which may be in the app's own.
    let written = match host {
        "vercel" => out.join(".vercel/output"),
        _ => out.to_path_buf(),
    };
    crate::deploy::check_out(root, &written)?;
    let (target, what) = match host {
        "lambda" => (LAMBDA_TARGET, "Linux x86_64"),
        _ => (WASM_TARGET, "WebAssembly"),
    };
    let sysroot = std::process::Command::new("rustc")
        .args(["--print", "sysroot"])
        .output()
        .map_err(|e| {
            format!("Could not run rustc: {e}.\nInstall Rust from https://rustup.rs, and open a new terminal.")
        })?;
    let sysroot = String::from_utf8_lossy(&sysroot.stdout).trim().to_string();
    if !Path::new(&sysroot)
        .join("lib/rustlib")
        .join(target)
        .is_dir()
    {
        return Err(format!(
            "The {host} build needs Rust's {target} target.\nInstall it with rustup target add {target}, then run this again."
        ));
    }
    let imports = crate::check(root)?;
    css::build(root)?;
    images::build(root);
    npm::vendor(root, &imports)?;
    let started = Instant::now();
    term::step(&format!("Building for {host} ({what})"));
    // Linked by Rust's own lld, which needs no C toolchain for Linux on any
    // machine, and stripped: Lambda loads it on every cold start.
    const LINKER: &str = "CARGO_TARGET_X86_64_UNKNOWN_LINUX_MUSL_LINKER";
    let strip = ("CARGO_PROFILE_RELEASE_STRIP", "symbols");
    // Pages bundles the module into a worker with a size limit: the smallest.
    let small = ("CARGO_PROFILE_RELEASE_OPT_LEVEL", "z");
    let env: &[(&str, &str)] = match host {
        "pages" => &[small],
        "lambda" if std::env::var_os(LINKER).is_none() => &[(LINKER, "rust-lld"), strip],
        "lambda" => &[strip],
        _ => &[],
    };
    let b = cargo::build_for(root, true, false, &["--target", target], env);
    let app = b
        .exe
        .filter(|_| b.ok)
        .ok_or("The build failed.\nThe compiler's errors are above.")?;
    let app = std::fs::read(&app).map_err(|e| format!("{}: {e}", app.display()))?;
    let package = cargo::package_name(root).ok_or("Cargo.toml has no package name.")?;
    let has_static = root.join("static").is_dir();
    let mut layout = layout(host, &package, app, has_static)?;
    if host == "pages" {
        let routes = routes(&root.join("static"));
        layout.files.push(("_routes.json", routes.into_bytes()));
    }

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

/// `wasm` is the built app: `app.wasm`, or for Lambda its Linux binary.
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
        "pages" => Layout {
            files: vec![("_worker.js", pages_worker().into_bytes()), ("app.wasm", wasm)],
            statics: has_static.then_some(""),
            deploy: "npx wrangler pages deploy .",
        },
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
        "bun" => Layout {
            files: vec![
                ("server.mjs", text(BUN)),
                ("bridge.mjs", bridge()),
                ("app.wasm", wasm),
                ("package.json", format!("{{\n  \"name\": \"{package}\",\n  \"private\": true,\n  \"type\": \"module\",\n  \"scripts\": {{ \"start\": \"bun server.mjs\" }}\n}}\n").into_bytes()),
            ],
            statics: None,
            deploy: "bun server.mjs runs it",
        },
        "lambda" => Layout {
            files: vec![("bootstrap.zip", zip("bootstrap", &wasm))],
            statics: None,
            deploy: "aws lambda update-function-code --function-name <name> --zip-file fileb://bootstrap.zip",
        },
        _ => return Err(format!("There is no target {host}.\nThe targets are {}, native, static and docker.", HOSTS.join(", "))),
    })
}

/// Pages' `_worker.js`: one module, the worker's entry and the bridge, which
/// imports `app.wasm` for the host to compile.
fn pages_worker() -> String {
    let entry: Vec<_> = WORKER
        .lines()
        .filter(|l| !l.starts_with("import { wisp }"))
        .collect();
    let bridge = BRIDGE.replacen("export function wisp(", "function wisp(", 1);
    let at = entry
        .iter()
        .position(|l| l.starts_with("let "))
        .unwrap_or(0);
    format!(
        "{}\n{bridge}\n{}\n",
        entry[..at].join("\n"),
        entry[at..].join("\n")
    )
}

/// Pages' `_routes.json`: everything reaches the worker but the app's static
/// files, which its CDN answers.
fn routes(statics: &Path) -> String {
    let mut skip = Vec::new();
    for e in std::fs::read_dir(statics).into_iter().flatten().flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        let all = if e.path().is_dir() { "/*" } else { "" };
        skip.push(format!("\"/{name}{all}\""));
    }
    skip.sort();
    format!(
        "{{\"version\":1,\"include\":[\"/*\"],\"exclude\":[{}]}}\n",
        skip.join(",")
    )
}

/// Standard base64 with padding, for the wasm inlined into Netlify's bundle.
fn base64(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    wisp_shared::base64::encode(&mut out, bytes, false);
    out
}

/// A zip of one executable file, `name`, stored as it is: Lambda's
/// `bootstrap`. Its mode is in the Unix attributes, so Lambda may run it.
fn zip(name: &str, data: &[u8]) -> Vec<u8> {
    fn fields(z: &mut Vec<u8>, f: &[(u32, usize)]) {
        for &(v, width) in f {
            z.extend_from_slice(&v.to_le_bytes()[..width]);
        }
    }
    let (crc, size, len) = (crc32(data), data.len() as u32, name.len() as u32);
    let mut z = Vec::with_capacity(data.len() + 2 * name.len() + 100);
    // Local header: version 2.0 needed, no flags, stored, 1980-01-01.
    let head = [(0x04034b50, 4), (20, 2), (0, 2), (0, 2), (0, 2), (0x21, 2)];
    let sizes = [(crc, 4), (size, 4), (size, 4), (len, 2), (0, 2)];
    fields(&mut z, &head);
    fields(&mut z, &sizes);
    z.extend_from_slice(name.as_bytes());
    z.extend_from_slice(data);
    // Central directory: made on Unix (3), the same, then mode rwxr-xr-x.
    let at = z.len() as u32;
    fields(&mut z, &[(0x02014b50, 4), (3 << 8 | 20, 2)]);
    fields(&mut z, &head[1..]);
    fields(&mut z, &sizes);
    fields(
        &mut z,
        &[(0, 2), (0, 2), (0, 2), (0o100755 << 16, 4), (0, 4)],
    );
    z.extend_from_slice(name.as_bytes());
    let dir = z.len() as u32 - at;
    fields(
        &mut z,
        &[
            (0x06054b50, 4),
            (0, 2),
            (0, 2),
            (1, 2),
            (1, 2),
            (dir, 4),
            (at, 4),
            (0, 2),
        ],
    );
    z
}

/// CRC-32 (IEEE), a bit at a time: once per build.
fn crc32(data: &[u8]) -> u32 {
    let mut crc = !0u32;
    for &b in data {
        crc ^= b as u32;
        for _ in 0..8 {
            crc = crc >> 1 ^ 0xEDB8_8320 & (crc & 1).wrapping_neg();
        }
    }
    !crc
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
        for host in HOSTS
            .into_iter()
            .filter(|h| !["lambda", "pages"].contains(h))
        {
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
    fn pages_is_one_worker_module_that_imports_the_wasm() {
        let w = pages_worker();
        assert!(w.contains("import module from './app.wasm'"));
        assert!(w.contains("\nfunction wisp(") && !w.contains("export function wisp"));
        assert!(!w.contains("from './bridge.mjs'") && w.contains("export default {"));
        let dir = std::env::temp_dir().join(format!("wisp-routes-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("img")).unwrap();
        std::fs::write(dir.join("a.css"), "").unwrap();
        let r = routes(&dir);
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(
            r,
            "{\"version\":1,\"include\":[\"/*\"],\"exclude\":[\"/a.css\",\"/img/*\"]}\n"
        );
    }

    #[test]
    fn lambda_gets_an_executable_bootstrap_zip() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        let l = layout("lambda", "site", b"\x7fELF".to_vec(), true).unwrap();
        let (path, z) = &l.files[0];
        assert_eq!(*path, "bootstrap.zip");
        assert!(z.starts_with(b"PK\x03\x04") && z.windows(13).any(|w| w == b"bootstrap\x7fELF"));
        let end = &z[z.len() - 22..];
        assert!(end.starts_with(b"PK\x05\x06"));
        let at = u32::from_le_bytes(end[16..20].try_into().unwrap()) as usize;
        assert_eq!(&z[at..at + 4], b"PK\x01\x02");
        assert_eq!(&z[at + 38..at + 42], &(0o100755u32 << 16).to_le_bytes());
    }

    #[test]
    fn a_hosts_build_picks_its_target() {
        let env = |set: &'static [(&'static str, &'static str)]| {
            move |k: &str| {
                set.iter()
                    .find(|(n, _)| *n == k)
                    .map(|(_, v)| v.to_string())
            }
        };
        assert_eq!(detect(env(&[("VERCEL", "1")])), Some(("VERCEL", "vercel")));
        assert_eq!(
            detect(env(&[("CF_PAGES", "1")])).map(|d| d.1),
            Some("cloudflare")
        );
        assert_eq!(
            detect(env(&[("NETLIFY", "true")])).map(|d| d.1),
            Some("netlify")
        );
        assert_eq!(detect(env(&[("NETLIFY", "false"), ("CI", "1")])), None);
        assert_eq!(detect(env(&[])), None);
    }

    #[test]
    fn shims_import_the_bridge_they_ship_with() {
        for shim in [NODE, WORKER, DENO, NETLIFY, BUN] {
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
