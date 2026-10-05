//! An installable, offline app: the service worker (`src/service-worker.js`
//! or `.ts`, or Wisp's own with `"offline": true`), served at
//! `/service-worker.js` and registered by every page, and the web app
//! manifest (`src/manifest.json`, or `wisp::app_manifest` in `init`),
//! served at `/manifest.webmanifest` and linked, with `static/`'s icons.
//! An app with neither gets nothing: no tag, no route, no byte.

use crate::image;
use crate::protocol::{IMAGES, MANIFEST_PATH, SERVICE_WORKER_PATH};
use crate::{fnv1a, js, json_str};
use std::path::Path;

pub(crate) struct Pwa {
    /// The service worker, or "".
    pub worker: String,
    /// The manifest as served: `None` when `init` gives it, "" without one.
    pub manifest: Option<String>,
    /// `static/`'s icons, as the manifest's `icons` JSON.
    pub icons: String,
    /// What every page's head gets.
    pub head: String,
    /// The CSP hash of the script that registers the worker.
    pub hash: Option<String>,
}

pub(crate) struct Input<'a> {
    pub root: &'a Path,
    /// The browser files' versioned URLs, and `static/`'s URLs with their
    /// hashes: a release build's. A dev build's lists are empty, as
    /// nothing it serves is worth keeping.
    pub build: Vec<String>,
    pub files: Vec<(String, String)>,
    /// `src/hooks.rs` calls `wisp::app_manifest`.
    pub runtime_manifest: bool,
    /// The `PUBLIC_*` variables, for the worker's `env.PUBLIC_X`.
    pub env: &'a [(String, String)],
    /// The path the app is served under (`WISP_BASE`), or "".
    pub base: &'a str,
}

/// The script every page runs to register the worker, served under `base`.
fn register(base: &str) -> String {
    format!("navigator.serviceWorker?.register(\"{base}{SERVICE_WORKER_PATH}\")")
}

/// Wisp's own service worker, after `build`, `files` and `version`: the
/// shell (`/`), the browser files and `static/` kept at install; a file of
/// `build` from what was kept; a page from the network, kept, and offline
/// the kept page or a short offline page; anything else from the network,
/// or what was kept. `root` is the app's front page (`/`, under a base path
/// `/app/`).
const OFFLINE_WORKER: &str = r#"const cache = 'wisp-' + version;
const offline = '<!doctype html><meta charset="utf-8"><meta name="viewport" content="width=device-width"><title>Offline</title><p style="font:1.1rem system-ui;text-align:center;margin-top:30vh">You are offline. This page loads when you are back.</p>';
self.addEventListener('install', (e) => {
  e.waitUntil(caches.open(cache).then((c) => Promise.allSettled([root, ...build, ...files].map((u) => c.add(u)))).then(() => self.skipWaiting()));
});
self.addEventListener('activate', (e) => {
  e.waitUntil(caches.keys().then((ks) => Promise.all(ks.filter((k) => k.startsWith('wisp-') && k != cache).map((k) => caches.delete(k)))).then(() => self.clients.claim()));
});
self.addEventListener('fetch', (e) => {
  const r = e.request;
  const url = new URL(r.url);
  if (r.method != 'GET' || url.origin != location.origin || url.pathname.startsWith(root + '_wisp/') || r.headers.get('accept') == 'text/event-stream') return;
  const page = r.mode == 'navigate' || (r.headers.get('accept') || '').includes('text/html');
  e.respondWith(caches.open(cache).then(async (c) => {
    const kept = await c.match(r);
    if (kept && build.includes(url.pathname + url.search)) return kept;
    try {
      const got = await fetch(r);
      if (got.ok && (page || kept)) c.put(r, got.clone());
      return got;
    } catch (err) {
      if (kept) return kept;
      if (page) return new Response(offline, { status: 503, headers: { 'content-type': 'text/html; charset=utf-8' } });
      throw err;
    }
  }));
});
"#;

/// The app's worker and manifest, or `None` when it has neither.
pub(crate) fn build(i: &Input) -> Result<Option<Pwa>, String> {
    let src = i.root.join("src");
    let manifest_src = crate::read_source(&src.join("manifest.json")).ok();
    let icons = icons(i.root, i.base);
    let (manifest, offline) = match &manifest_src {
        Some(text) => {
            let (m, offline) = wisp_shared::manifest::complete(text, &icons, i.base)
                .map_err(|e| format!("src/manifest.json: {e}"))?;
            (Some(m), offline)
        }
        None if i.runtime_manifest => (None, false),
        None => (Some(String::new()), false),
    };
    let own = ["service-worker.js", "service-worker.ts"]
        .into_iter()
        .filter(|f| src.join(f).is_file())
        .collect::<Vec<_>>();
    if own.len() > 1 {
        return Err("src/service-worker.js and src/service-worker.ts: keep one".into());
    }
    let mut version = Vec::new();
    for u in &i.build {
        version.extend_from_slice(u.as_bytes());
        version.push(0);
    }
    for (u, etag) in &i.files {
        version.extend_from_slice(format!("{u}\0{etag}\0").as_bytes());
    }
    let lists = |body: &str| {
        let mut v = version.clone();
        v.extend_from_slice(body.as_bytes());
        let list = |us: &mut dyn Iterator<Item = &String>| {
            let all: Vec<String> = us.map(|u| json_str(u)).collect();
            format!("[{}]", all.join(", "))
        };
        (
            list(&mut i.build.iter()),
            list(&mut i.files.iter().map(|f| &f.0)),
            json_str(&format!("{:016x}", fnv1a(&v))),
        )
    };
    let worker = match own.first() {
        Some(f) => {
            let rel = format!("src/{f}");
            let text = crate::read_source(&src.join(f)).map_err(|e| format!("{rel}: {e}"))?;
            worker(&text, &rel, i.env, lists)?
        }
        None if offline => {
            let (build, files, version) = lists(OFFLINE_WORKER);
            format!(
                "// Wisp's offline service worker.\nconst build = {build}, files = {files}, version = {version}, root = {};\n{OFFLINE_WORKER}",
                json_str(&format!("{}/", i.base))
            )
        }
        None => String::new(),
    };
    if worker.is_empty() && manifest.as_deref() == Some("") {
        return Ok(None);
    }
    let mut head = String::new();
    if manifest.as_deref() != Some("") {
        head.push_str(&format!(
            "<link rel=\"manifest\" href=\"{}{MANIFEST_PATH}\">",
            i.base
        ));
    }
    let hash = (!worker.is_empty()).then(|| {
        let register = register(i.base);
        head.push_str(&format!("<script>{register}</script>"));
        crate::csp::hash(&register)
    });
    Ok(Some(Pwa {
        worker,
        manifest,
        icons,
        head,
        hash,
    }))
}

/// The worker `src` (at `rel`) as a classic script: TypeScript's types
/// gone, `env.PUBLIC_X` filled in, and its imports of `'wisp/sw'` made
/// constants of what `lists` gives (`build`, `files`, `version`, as JS).
/// A worker loads no module but that one: any other import is an error.
fn worker(
    src: &str,
    rel: &str,
    env: &[(String, String)],
    lists: impl Fn(&str) -> (String, String, String),
) -> Result<String, String> {
    let line = |off: usize| src[..off.min(src.len())].matches('\n').count() + 1;
    let code = if rel.ends_with(".ts") {
        js::strip_types(src).map_err(|(off, msg)| format!("{rel}:{}: {msg}", line(off)))?
    } else {
        src.to_string()
    };
    let code = js::public_env(&code, &|n| {
        env.iter().find(|(k, _)| k == n).map(|(_, v)| v.clone())
    })
    .map_err(|(off, msg)| format!("{rel}:{}: {msg}", line(off)))?;
    let (build, files, version) = lists(&code);
    let mut out = String::with_capacity(code.len() + build.len() + files.len());
    let mut at = 0;
    for (a, b) in js::imports(&code) {
        let stmt = &code[a..b];
        let err = |msg: &str| format!("{rel}:{}: {msg}", line(a));
        let from = stmt
            .rfind(['\'', '"'])
            .and_then(|end| {
                let q = stmt.as_bytes()[end] as char;
                stmt[..end].rfind(q).map(|start| &stmt[start + 1..end])
            })
            .unwrap_or("");
        if from != "wisp/sw" {
            return Err(err(&format!(
                "a service worker imports only 'wisp/sw' (its `build`, `files` and `version`), not '{from}': it runs as a classic script"
            )));
        }
        let what = stmt["import".len()..]
            .rsplit_once("from")
            .map_or("", |(w, _)| w)
            .trim();
        let names = if let Some(alias) = what.strip_prefix('*') {
            let alias = alias.trim().strip_prefix("as").unwrap_or("").trim();
            format!("{alias} = {{ build: {build}, files: {files}, version: {version} }}")
        } else {
            let inner = what
                .strip_prefix('{')
                .and_then(|w| w.strip_suffix('}'))
                .ok_or_else(|| {
                    err("import what 'wisp/sw' has by name: import { build, files, version } from 'wisp/sw'")
                })?;
            let mut parts = Vec::new();
            for n in inner.split(',').map(str::trim).filter(|n| !n.is_empty()) {
                let (name, local) = n.split_once(" as ").unwrap_or((n, n));
                let (name, local) = (name.trim(), local.trim());
                let value = match name {
                    "build" => &build,
                    "files" => &files,
                    "version" => &version,
                    _ => {
                        return Err(err(&format!(
                            "'wisp/sw' has `build`, `files` and `version`, not `{name}`"
                        )));
                    }
                };
                parts.push(format!("{local} = {value}"));
            }
            parts.join(", ")
        };
        out.push_str(&code[at..a]);
        out.push_str("const ");
        out.push_str(&names);
        out.push(';');
        // The statement's lines stay lines.
        out.push_str(&"\n".repeat(stmt.matches('\n').count()));
        at = b;
    }
    out.push_str(&code[at..]);
    Ok(out)
}

/// The app's icons as a manifest's `icons`: each `static/icon*.png` with
/// its size from its header, each `static/icon*.svg` of any size, and the
/// 192 and 512 px WebP widths `wisp build` wrote of `static/icon.png`
/// (with cwebp, as for images; when they are there).
fn icons(root: &Path, base: &str) -> String {
    let dir = root.join("static");
    let mut names: Vec<String> = (std::fs::read_dir(&dir).into_iter().flatten())
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with("icon") && (n.ends_with(".png") || n.ends_with(".svg")))
        .collect();
    names.sort();
    let icon = |src: &str, sizes: &str, ty: &str| {
        format!(
            "{{\"src\":{},\"sizes\":\"{sizes}\",\"type\":\"{ty}\"}}",
            json_str(src)
        )
    };
    let mut out = Vec::new();
    for n in names {
        let src = format!("{base}/{}", crate::codegen::encode_path(&n));
        if n.ends_with(".svg") {
            out.push(icon(&src, "any", "image/svg+xml"));
        } else if let Some(s) = std::fs::read(dir.join(&n))
            .ok()
            .and_then(|b| image::size(&b))
        {
            out.push(icon(
                &src,
                &format!("{}x{}", s.width, s.height),
                "image/png",
            ));
        }
    }
    if let Some((size, widths)) = image::icon(root) {
        for (w, name) in widths
            .iter()
            .filter(|(_, n)| root.join(image::DIR).join(n).is_file())
        {
            // cwebp keeps the aspect.
            let h = (u64::from(size.height) * u64::from(*w) / u64::from(size.width)).max(1);
            out.push(icon(
                &format!("{IMAGES}{name}"),
                &format!("{w}x{h}"),
                "image/webp",
            ));
        }
    }
    format!("[{}]", out.join(","))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::SERVICE_WORKER_PATH;

    fn lists(_: &str) -> (String, String, String) {
        (
            "[\"/_app/a.js?v=1\"]".into(),
            "[\"/logo.png\"]".into(),
            "\"v1\"".into(),
        )
    }

    #[test]
    fn the_worker_gets_its_lists() {
        let src = "import { build, files as statics, version } from 'wisp/sw';\nconst x = env.PUBLIC_A\nself.x = [build, statics, version, x]\n";
        let env = [("PUBLIC_A".to_string(), "a".to_string())];
        let out = worker(src, "src/service-worker.js", &env, lists).unwrap();
        assert_eq!(
            out,
            "const build = [\"/_app/a.js?v=1\"], statics = [\"/logo.png\"], version = \"v1\";\nconst x = \"a\"\nself.x = [build, statics, version, x]\n"
        );
        let out = worker(
            "import * as sw from \"wisp/sw\"\nlet n: number = 1",
            "src/service-worker.ts",
            &[],
            lists,
        )
        .unwrap();
        assert_eq!(
            out,
            "const sw = { build: [\"/_app/a.js?v=1\"], files: [\"/logo.png\"], version: \"v1\" };\nlet n         = 1"
        );
    }

    #[test]
    fn the_worker_imports_nothing_else() {
        let err = |src: &str| worker(src, "src/service-worker.js", &[], lists).unwrap_err();
        assert!(
            err("\nimport x from './x.js'")
                .contains("src/service-worker.js:2: a service worker imports only 'wisp/sw'"),
            "{}",
            err("\nimport x from './x.js'")
        );
        assert!(err("import { cache } from 'wisp/sw'").contains("not `cache`"));
        assert!(err("import sw from 'wisp/sw'").contains("by name"));
    }

    fn app(files: &[(&str, &str)], runtime_manifest: bool) -> Option<Pwa> {
        built(files, runtime_manifest).unwrap()
    }

    fn try_app(files: &[(&str, &str)]) -> String {
        built(files, false).err().unwrap_or_default()
    }

    /// An app of `files`, built: what `build` makes of it.
    fn built(files: &[(&str, &str)], runtime_manifest: bool) -> Result<Option<Pwa>, String> {
        static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!("wisp-pwa-{}-{n}", std::process::id()));
        for (path, text) in files {
            let f = root.join(path);
            std::fs::create_dir_all(f.parent().unwrap()).unwrap();
            std::fs::write(f, text).unwrap();
        }
        let out = build(&Input {
            root: &root,
            build: vec!["/_app/wisp.js?v=1".into()],
            files: vec![("/a.txt".into(), "ab".into())],
            runtime_manifest,
            env: &[],
            base: "",
        });
        let _ = std::fs::remove_dir_all(&root);
        out
    }

    #[test]
    fn an_app_with_neither_gets_nothing() {
        assert!(app(&[("src/routes/+page.wisp", "x")], false).is_none());
    }

    #[test]
    fn offline_is_wisps_worker() {
        let p = app(
            &[("src/manifest.json", r#"{"name":"N","offline":true}"#)],
            false,
        )
        .unwrap();
        assert!(p.worker.starts_with(
            "// Wisp's offline service worker.\nconst build = [\"/_app/wisp.js?v=1\"], files = [\"/a.txt\"], version = \""
        ));
        assert!(p.worker.ends_with(OFFLINE_WORKER));
        assert_eq!(
            p.head,
            format!(
                "<link rel=\"manifest\" href=\"/manifest.webmanifest\"><script>{}</script>",
                register("")
            )
        );
        assert_eq!(p.hash, Some(crate::csp::hash(&register(""))));
        assert!(register("").contains(&format!("\"{SERVICE_WORKER_PATH}\"")));
        assert!(!p.manifest.unwrap().contains("offline"));
    }

    #[test]
    fn a_worker_or_a_manifest_alone() {
        let p = app(
            &[(
                "src/service-worker.js",
                "import { version } from 'wisp/sw'\nself.v = version",
            )],
            false,
        )
        .unwrap();
        assert!(p.worker.starts_with("const version = \""), "{}", p.worker);
        assert_eq!(p.manifest.as_deref(), Some(""));
        assert_eq!(p.head, format!("<script>{}</script>", register("")));
        // `wisp::app_manifest` in `init`: linked, made at startup.
        let p = app(&[], true).unwrap();
        assert_eq!(p.manifest, None);
        assert!(p.worker.is_empty() && p.hash.is_none());
        assert_eq!(
            p.head,
            "<link rel=\"manifest\" href=\"/manifest.webmanifest\">"
        );
    }

    /// Under a base path every URL the app writes is under it: the
    /// manifest, the worker's registration, the icons, the front page.
    #[test]
    fn a_base_path_goes_before_every_url() {
        let root = std::env::temp_dir().join(format!("wisp-pwa-base-{}", std::process::id()));
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::create_dir_all(root.join("static")).unwrap();
        std::fs::write(
            root.join("src/manifest.json"),
            r#"{"name":"N","offline":true}"#,
        )
        .unwrap();
        std::fs::write(root.join("static/icon.svg"), "<svg/>").unwrap();
        let out = build(&Input {
            root: &root,
            build: vec![],
            files: vec![],
            runtime_manifest: false,
            env: &[],
            base: "/app",
        });
        let _ = std::fs::remove_dir_all(&root);
        let p = out.unwrap().unwrap();
        assert_eq!(
            p.head,
            "<link rel=\"manifest\" href=\"/app/manifest.webmanifest\"><script>navigator.serviceWorker?.register(\"/app/service-worker.js\")</script>"
        );
        assert_eq!(p.hash, Some(crate::csp::hash(&register("/app"))));
        let m = p.manifest.unwrap();
        assert!(m.contains("\"start_url\":\"/app/\""), "{m}");
        assert!(m.contains("\"src\":\"/app/icon.svg\""), "{m}");
        assert!(p.worker.contains("root = \"/app/\""), "{}", p.worker);
    }

    #[test]
    fn mistakes() {
        let both = [("src/service-worker.js", ""), ("src/service-worker.ts", "")];
        assert!(try_app(&both).contains("keep one"));
        let bad = [("src/manifest.json", "[]")];
        assert!(
            try_app(&bad).starts_with("src/manifest.json: a web app manifest is a JSON object")
        );
    }

    #[test]
    fn icons_and_their_sizes() {
        let root = std::env::temp_dir().join(format!("wisp-pwa-icons-{}", std::process::id()));
        let mut png = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec();
        png.extend_from_slice(&1024u32.to_be_bytes());
        png.extend_from_slice(&512u32.to_be_bytes());
        png.extend_from_slice(&[8, 6, 0, 0, 0]);
        let hash = image::hash(&png);
        for (path, bytes) in [
            ("src/manifest.json", &b"{}"[..]),
            ("static/icon.png", &png),
            ("static/icon.svg", b"<svg/>"),
            ("static/logo.png", &png),
            // `wisp build` wrote one width so far.
            (&format!(".wisp/img/{hash}-192.webp"), b"RIFF"),
        ] {
            let f = root.join(path);
            std::fs::create_dir_all(f.parent().unwrap()).unwrap();
            std::fs::write(f, bytes).unwrap();
        }
        let got = icons(&root, "");
        let _ = std::fs::remove_dir_all(&root);
        assert_eq!(
            got,
            format!(
                "[{{\"src\":\"/icon.png\",\"sizes\":\"1024x512\",\"type\":\"image/png\"}},\
                 {{\"src\":\"/icon.svg\",\"sizes\":\"any\",\"type\":\"image/svg+xml\"}},\
                 {{\"src\":\"/_app/img/{hash}-192.webp\",\"sizes\":\"192x96\",\"type\":\"image/webp\"}}]"
            )
        );
    }
}
