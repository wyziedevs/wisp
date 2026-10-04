//! `wisp build --target <host>`: the app built for WebAssembly
//! (`wasm32-unknown-unknown`, see `wisp::edge`) and a folder that host
//! deploys as it is: `app.wasm`, a small entry shim over the shared
//! `bridge.mjs`, the host's config and the static files for its CDN. For
//! `lambda`, the app's own binary for Linux, zipped as Lambda's `bootstrap`.

use crate::{cargo, css, deploy, images, npm, term};
use jobs::Jobs;
use std::path::Path;
use std::time::Instant;
use wisp_build::routes::Seg;

mod runtime;

mod jobs;

const BRIDGE: &str = include_str!("bridge.js");
const NODE: &str = include_str!("node.mjs");
const WORKER: &str = include_str!("worker.js");
const DENO: &str = include_str!("deno.ts");
const NETLIFY: &str = include_str!("netlify.mjs");
const VERCEL_EDGE: &str = include_str!("vercel_edge.mjs");
const NETLIFY_EDGE: &str = include_str!("netlify_edge.mjs");
const BUN: &str = include_str!("bun.mjs");

/// Vercel's `config.json`: static files first, then the one function, and
/// the app's cron schedules, which Vercel requests by their path.
fn vercel_config(jobs: &Jobs, to_edge: &str) -> String {
    let crons: Vec<_> = jobs
        .triggers()
        .into_iter()
        .map(|e| format!(r#"{{"path":"{}","schedule":"{e}"}}"#, jobs::path(e)))
        .collect();
    let crons = match crons.is_empty() {
        true => String::new(),
        false => format!(r#","crons":[{}]"#, crons.join(",")),
    };
    format!(
        r#"{{"version":3,"routes":[{{"handle":"filesystem"}},{to_edge}{{"src":"/(.*)","dest":"/index"}}]{crons}}}"#
    )
}

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

pub fn build(root: &Path, host: &str, edge: bool, out: &Path) -> Result<(), String> {
    // Vercel's folder is `.vercel/output`, which may be in the app's own.
    let written = match host {
        "vercel" => out.join(".vercel/output"),
        _ => out.to_path_buf(),
    };
    crate::deploy::check_out(root, &written)?;
    let jobs = jobs::scan(root)?;
    jobs::check(host, edge, &jobs)?;
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
    // Routes with `RUNTIME = Edge`: only a host with both runtimes has any
    // use for them, and `--edge` puts every route there.
    let edges = match host {
        "vercel" | "netlify" if !edge => runtime::edge_routes(root)?,
        _ => Vec::new(),
    };
    css::build(root)?;
    images::build(root);
    npm::vendor(root, &imports)?;
    let started = Instant::now();
    term::step(&format!("Building for {host} ({what})"));
    // Linked by Rust's own lld, which needs no C toolchain for Linux on any
    // machine, and stripped (every build): a host loads it on each cold start.
    const LINKER: &str = "CARGO_TARGET_X86_64_UNKNOWN_LINUX_MUSL_LINKER";
    let strip = ("CARGO_PROFILE_RELEASE_STRIP", "symbols");
    // The fastest code (opt-level 3, about 180 KB gzipped) fits every limit but
    // Vercel's and Netlify's edge functions: `s` is as small as `z`, and faster.
    let env: &[(&str, &str)] = match host {
        _ if edge => &[("CARGO_PROFILE_RELEASE_OPT_LEVEL", "s"), strip],
        "lambda" if std::env::var_os(LINKER).is_none() => &[(LINKER, "rust-lld"), strip],
        _ => &[strip],
    };
    let b = cargo::build_for(root, true, false, &["--target", target], env);
    let app = b
        .exe
        .filter(|_| b.ok)
        .ok_or("The build failed.\nThe compiler's errors are above.")?;
    let app = std::fs::read(&app).map_err(|e| format!("{}: {e}", app.display()))?;
    // The same app again for the edge function, built smaller.
    let edge_app = match edges.is_empty() {
        true => None,
        false => {
            term::step(&format!(
                "Building the edge function for {} routes",
                edges.len()
            ));
            let env = [("CARGO_PROFILE_RELEASE_OPT_LEVEL", "s"), strip];
            let b = cargo::build_for(root, true, false, &["--target", target], &env);
            let exe = b
                .exe
                .filter(|_| b.ok)
                .ok_or("The edge build failed.\nThe compiler's errors are above.")?;
            Some(std::fs::read(&exe).map_err(|e| format!("{}: {e}", exe.display()))?)
        }
    };
    let package = cargo::package_name(root).ok_or("Cargo.toml has no package name.")?;
    let has_static = root.join("static").is_dir();
    let skips = skips(&root.join("static"));
    let mut layout = layout(host, &package, app, has_static, edge, &skips, &jobs)?;
    if let Some(wasm) = edge_app {
        add_edge(&mut layout, host, wasm, &skips, &edges, &jobs);
    }
    if host == "pages" {
        layout
            .files
            .push(("_routes.json", routes(&skips).into_bytes()));
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
    if jobs.any() {
        println!(
            "    Set CRON_SECRET there (any long random text): the host's cron triggers send it."
        );
        if jobs.work {
            println!(
                "    Queued jobs are kept in the app's tables: set WISP_STORE there too, or they are lost."
            );
        }
    }
    Ok(())
}

/// `wasm` is the built app: `app.wasm`, or for Lambda its Linux binary.
/// `edge` is `--edge` (Vercel, Netlify); `skips` the static paths; `jobs`
/// what the app asks of the host's cron (see [`jobs::check`]).
fn layout(
    host: &str,
    package: &str,
    wasm: Vec<u8>,
    has_static: bool,
    edge: bool,
    skips: &[String],
    jobs: &Jobs,
) -> Result<Layout, String> {
    let bridge = || BRIDGE.as_bytes().to_vec();
    let text = |s: &str| s.as_bytes().to_vec();
    Ok(match host {
        "cloudflare" => {
            let assets = if has_static { "\n# Served before the worker runs.\n[assets]\ndirectory = \"public\"\n" } else { "" };
            let triggers: Vec<_> = jobs.triggers().iter().map(|e| format!("\"{e}\"")).collect();
            let triggers = match triggers.is_empty() {
                true => String::new(),
                false => format!("\n# The app's wisp::cron schedules, and each minute for its queues.\n[triggers]\ncrons = [{}]\n", triggers.join(", ")),
            };
            let wrangler = format!("name = \"{package}\"\nmain = \"worker.js\"\ncompatibility_date = \"2025-09-01\"\n{triggers}{assets}");
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
        "vercel" if edge => Layout {
            files: vec![
                (".vercel/output/config.json", vercel_config(jobs, "").into_bytes()),
                (".vercel/output/functions/index.func/.vc-config.json", text(r#"{"runtime":"edge","entrypoint":"index.mjs"}"#)),
                (".vercel/output/functions/index.func/index.mjs", text(VERCEL_EDGE)),
                (".vercel/output/functions/index.func/bridge.mjs", bridge()),
                (".vercel/output/functions/index.func/app.wasm", wasm),
            ],
            statics: Some(".vercel/output/static"),
            deploy: "npx vercel deploy --prebuilt",
        },
        "netlify" if edge => {
            let list: Vec<_> = skips.iter().map(|s| format!("'{s}'")).collect();
            Layout {
                files: vec![
                    ("netlify.toml", text("[build]\npublish = \"public\"\n")),
                    ("netlify/edge-functions/wisp.mjs", NETLIFY_EDGE.replace("/*SKIP*/", &list.join(", ")).into_bytes()),
                    ("netlify/edge-functions/bridge.mjs", bridge()),
                    ("netlify/edge-functions/app.wasm", wasm),
                ],
                statics: Some("public"),
                deploy: "npx netlify deploy --prod",
            }
        }
        "vercel" => {
            Layout {
                files: vec![
                    (".vercel/output/config.json", vercel_config(jobs, "").into_bytes()),
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
        "netlify" => {
            let mut files = vec![
                ("netlify.toml", text("[build]\npublish = \"public\"\n\n[functions]\ndirectory = \"functions\"\n")),
                ("functions/wisp.mjs", text(NETLIFY)),
                ("functions/bridge.mjs", bridge()),
                ("functions/app.wasm.js", format!("export default \"{}\";\n", base64(&wasm)).into_bytes()),
            ];
            // A scheduled function has one schedule: one each, which asks the app.
            for (n, e) in jobs.triggers().into_iter().enumerate() {
                let path = format!("functions/wisp-cron-{n}.mjs");
                files.push((Box::leak(path.into_boxed_str()), netlify_cron(e).into_bytes()));
            }
            Layout { files, statics: Some("public"), deploy: "npx netlify deploy --prod" }
        }
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

/// A Netlify scheduled function: at `expr` it asks the app's function for
/// `/_wisp/cron/<expr>`, as the other hosts' triggers do.
fn netlify_cron(expr: &str) -> String {
    format!(
        "// Netlify scheduled function: runs the app's `wisp::cron` tasks of this schedule.\nimport app from './wisp.mjs';\n\nexport default async () => {{\n  const headers = {{ authorization: `Bearer ${{process.env.CRON_SECRET}}` }};\n  const res = await app(new Request('https://wisp.invalid{}', {{ headers }}), {{}});\n  if (!res.ok) throw new Error(`cron {expr}: ${{res.status}}`);\n}};\n\nexport const config = {{ schedule: '{expr}' }};\n",
        jobs::path(expr)
    )
}

/// Both runtimes from one app: the Node function `layout` has, and an edge
/// function for the routes `edges` (see `runtime`), which the host's routing
/// sends there. Static files are the CDN's either way.
fn add_edge(
    layout: &mut Layout,
    host: &str,
    wasm: Vec<u8>,
    skips: &[String],
    edges: &[Vec<Seg>],
    jobs: &Jobs,
) {
    let quoted = |f: fn(&[Seg]) -> String, q: char| -> Vec<String> {
        let mut v: Vec<_> = edges.iter().map(|s| f(s)).collect();
        v.sort();
        v.dedup();
        v.into_iter()
            .map(|p| format!("{q}{}{q}", p.replace('\\', "\\\\")))
            .collect()
    };
    let bridge = BRIDGE.as_bytes().to_vec();
    if host == "vercel" {
        let to_edge: String = quoted(runtime::vercel, '"')
            .iter()
            .map(|p| format!("{{\"src\":{p},\"dest\":\"/edge\"}},"))
            .collect();
        let config = vercel_config(jobs, &to_edge);
        layout
            .files
            .retain(|(p, _)| *p != ".vercel/output/config.json");
        layout.files.extend([
            (".vercel/output/config.json", config.into_bytes()),
            (
                ".vercel/output/functions/edge.func/.vc-config.json",
                br#"{"runtime":"edge","entrypoint":"index.mjs"}"#.to_vec(),
            ),
            (
                ".vercel/output/functions/edge.func/index.mjs",
                VERCEL_EDGE.as_bytes().to_vec(),
            ),
            (".vercel/output/functions/edge.func/bridge.mjs", bridge),
            (".vercel/output/functions/edge.func/app.wasm", wasm),
        ]);
    } else {
        let paths = quoted(runtime::netlify, '\'').join(", ");
        let skip: Vec<_> = skips.iter().map(|s| format!("'{s}'")).collect();
        let entry = NETLIFY_EDGE
            .replace("path: '/*'", &format!("path: [{paths}]"))
            .replace("/*SKIP*/", &skip.join(", "));
        layout.files.extend([
            ("netlify/edge-functions/wisp.mjs", entry.into_bytes()),
            ("netlify/edge-functions/bridge.mjs", bridge),
            ("netlify/edge-functions/app.wasm", wasm),
        ]);
    }
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

/// The paths of the app's static files, a folder as `/name/*`: what the
/// CDN answers, and the worker or function does not.
fn skips(statics: &Path) -> Vec<String> {
    let mut skip = Vec::new();
    for e in std::fs::read_dir(statics).into_iter().flatten().flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        let all = if e.path().is_dir() { "/*" } else { "" };
        skip.push(format!("/{name}{all}"));
    }
    skip.sort();
    skip
}

/// Pages' `_routes.json`: everything reaches the worker but the app's static
/// files.
fn routes(skips: &[String]) -> String {
    let skip: Vec<_> = skips.iter().map(|s| format!("\"{s}\"")).collect();
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

    fn text<'a>(l: &'a Layout, path: &str) -> std::borrow::Cow<'a, str> {
        let f = l.files.iter().find(|(p, _)| *p == path);
        String::from_utf8_lossy(&f.unwrap_or_else(|| panic!("no {path}")).1)
    }

    #[test]
    fn one_app_two_runtimes() {
        let edges = vec![
            vec![Seg::Static("blog".into()), Seg::Param("slug".into(), None)],
            vec![Seg::Static("api".into()), Seg::Rest("p".into())],
        ];
        let skips = ["/logo.png".to_string()];
        let mut v = layout(
            "vercel",
            "s",
            b"node".to_vec(),
            true,
            false,
            &skips,
            &Jobs::default(),
        )
        .unwrap();
        add_edge(
            &mut v,
            "vercel",
            b"edge".to_vec(),
            &skips,
            &edges,
            &Jobs::default(),
        );
        let config = text(&v, ".vercel/output/config.json");
        let want = r#"{"version":3,"routes":[{"handle":"filesystem"},{"src":"^/api(?:/.*)?/?$","dest":"/edge"},{"src":"^/blog/[^/]+/?$","dest":"/edge"},{"src":"/(.*)","dest":"/index"}]}"#;
        assert_eq!(config, want);
        assert_eq!(
            text(&v, ".vercel/output/functions/edge.func/.vc-config.json"),
            r#"{"runtime":"edge","entrypoint":"index.mjs"}"#
        );
        assert!(text(&v, ".vercel/output/functions/index.func/.vc-config.json").contains("nodejs"));
        assert_eq!(
            v.files
                .iter()
                .filter(|(p, _)| p.ends_with("config.json") && !p.contains("func"))
                .count(),
            1
        );
        let mut n = layout(
            "netlify",
            "s",
            b"node".to_vec(),
            true,
            false,
            &skips,
            &Jobs::default(),
        )
        .unwrap();
        add_edge(
            &mut n,
            "netlify",
            b"edge".to_vec(),
            &skips,
            &edges,
            &Jobs::default(),
        );
        let edge = text(&n, "netlify/edge-functions/wisp.mjs");
        assert!(
            edge.contains("path: ['/api/*', '/blog/:slug'], excludedPath: ['/logo.png']"),
            "{edge}"
        );
        assert!(text(&n, "functions/wisp.mjs").contains("path: '/*'"));
        assert!(n.files.iter().any(|(p, _)| *p == "functions/app.wasm.js"));
        // The routes sent to the edge function keep Vercel's crons.
        let mut v = layout(
            "vercel",
            "s",
            b"node".to_vec(),
            true,
            false,
            &skips,
            &jobs(),
        )
        .unwrap();
        add_edge(&mut v, "vercel", b"edge".to_vec(), &skips, &edges, &jobs());
        let config = text(&v, ".vercel/output/config.json");
        assert!(
            config.contains(r#""dest":"/edge"}"#)
                && config.contains(r#""crons":[{"path":"/_wisp/cron/0_3_*_*_*""#),
            "{config}"
        );
    }

    #[test]
    fn every_host_has_its_entry_and_the_app() {
        for host in HOSTS
            .into_iter()
            .filter(|h| !["lambda", "pages"].contains(h))
        {
            let l = layout(
                host,
                "site",
                b"\0asm".to_vec(),
                true,
                false,
                &[],
                &Jobs::default(),
            )
            .unwrap();
            let paths: Vec<_> = l.files.iter().map(|(p, _)| *p).collect();
            assert!(paths.iter().any(|p| p.ends_with("bridge.mjs")), "{host}");
            assert!(
                paths
                    .iter()
                    .any(|p| p.ends_with("app.wasm") || p.ends_with("app.wasm.js")),
                "{host}"
            );
        }
        let cf = layout(
            "cloudflare",
            "site",
            Vec::new(),
            true,
            false,
            &[],
            &Jobs::default(),
        )
        .unwrap();
        let wrangler = &cf
            .files
            .iter()
            .find(|(p, _)| *p == "wrangler.toml")
            .unwrap()
            .1;
        assert!(String::from_utf8_lossy(wrangler).contains("name = \"site\""));
        assert!(
            !String::from_utf8_lossy(
                &layout(
                    "cloudflare",
                    "site",
                    Vec::new(),
                    false,
                    false,
                    &[],
                    &Jobs::default()
                )
                .unwrap()
                .files[3]
                    .1
            )
            .contains("[assets]")
        );
        assert!(
            layout(
                "heroku",
                "site",
                Vec::new(),
                true,
                false,
                &[],
                &Jobs::default()
            )
            .is_err()
        );
    }

    /// The text of the file `path` in `host`'s folder.
    fn file(host: &str, edge: bool, jobs: &Jobs, path: &str) -> String {
        let l = layout(host, "site", Vec::new(), false, edge, &[], jobs).unwrap();
        let (_, bytes) = l.files.iter().find(|(p, _)| *p == path).unwrap();
        String::from_utf8(bytes.clone()).unwrap()
    }

    fn jobs() -> Jobs {
        Jobs {
            crons: vec!["0 3 * * *".into(), "*/5 * * * *".into()],
            work: true,
        }
    }

    #[test]
    fn cloudflare_gets_cron_triggers_and_a_worker_that_takes_them() {
        let wrangler = file("cloudflare", false, &jobs(), "wrangler.toml");
        assert!(
            wrangler
                .contains("[triggers]\ncrons = [\"0 3 * * *\", \"*/5 * * * *\", \"* * * * *\"]")
        );
        assert!(!file("cloudflare", false, &Jobs::default(), "wrangler.toml").contains("triggers"));
        let worker = file("cloudflare", false, &jobs(), "worker.js");
        assert!(
            worker.contains("async scheduled(event, env, ctx)") && worker.contains("/_wisp/cron/")
        );
        assert!(worker.contains(".join('_').replaceAll('/', '~')"));
    }

    #[test]
    fn vercel_gets_crons_in_its_config_on_both_runtimes() {
        let want = r#""crons":[{"path":"/_wisp/cron/0_3_*_*_*","schedule":"0 3 * * *"},{"path":"/_wisp/cron/*~5_*_*_*_*","schedule":"*/5 * * * *"},{"path":"/_wisp/cron/*_*_*_*_*","schedule":"* * * * *"}]}"#;
        for edge in [false, true] {
            let c = file("vercel", edge, &jobs(), ".vercel/output/config.json");
            assert!(
                c.starts_with(r#"{"version":3,"routes":["#) && c.ends_with(want),
                "{c}"
            );
        }
        let plain = file(
            "vercel",
            false,
            &Jobs::default(),
            ".vercel/output/config.json",
        );
        assert_eq!(
            plain,
            r#"{"version":3,"routes":[{"handle":"filesystem"},{"src":"/(.*)","dest":"/index"}]}"#
        );
    }

    #[test]
    fn netlify_gets_a_scheduled_function_for_each_schedule() {
        let f = file("netlify", false, &jobs(), "functions/wisp-cron-1.mjs");
        assert!(
            f.contains("schedule: '*/5 * * * *'")
                && f.contains("https://wisp.invalid/_wisp/cron/*~5_*_*_*_*")
        );
        assert!(f.contains("import app from './wisp.mjs'"));
        let l = layout(
            "netlify",
            "site",
            Vec::new(),
            false,
            false,
            &[],
            &Jobs::default(),
        )
        .unwrap();
        assert!(!l.files.iter().any(|(p, _)| p.contains("cron")));
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
        let r = routes(&skips(&dir));
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(
            r,
            "{\"version\":1,\"include\":[\"/*\"],\"exclude\":[\"/a.css\",\"/img/*\"]}\n"
        );
    }

    #[test]
    fn lambda_gets_an_executable_bootstrap_zip() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        let l = layout(
            "lambda",
            "site",
            b"\x7fELF".to_vec(),
            true,
            false,
            &[],
            &Jobs::default(),
        )
        .unwrap();
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
            assert!(BRIDGE.contains(&format!("exports.{export}")), "{export}");
        }
    }
}
