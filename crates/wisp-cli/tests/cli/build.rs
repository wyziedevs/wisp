//! `wisp build`: the release binary, a static site, a Dockerfile, and a
//! folder for each host.

use crate::{
    Dir, command, fail, fake_tailwind, has, new_app, pinned_app, read, tree, wisp, wisp_env, write,
};
use std::fs;
use std::path::Path;

fn assert_tree(dir: &Path, want: &[&str]) {
    let mut want = want.to_vec();
    want.sort_unstable();
    assert_eq!(tree(dir), want, "{}", dir.display());
}

#[test]
fn docker_files_are_written_once() {
    let cwd = Dir::new("docker");
    let app = new_app(&cwd, "site", &["-t", "minimal"]);
    let o = wisp(&app, &["build", "--docker"]);
    assert!(o.ok, "{}", o.err);
    has(
        &o.out,
        &[
            "Wrote Dockerfile",
            "Wrote .dockerignore",
            "docker build -t site .",
            "docker run -p 3000:3000 -e WISP_SECRET=... site",
            "Fly.io, Railway, Render, Cloud Run",
        ],
    );
    has(
        &read(&app, "Dockerfile"),
        &[
            "FROM rust:slim AS build",
            "cargo build --release && cp target/release/site /server",
            "ENV HOST=0.0.0.0 PORT=3000",
            "USER nobody",
            "CMD [\"server\"]",
        ],
    );
    has(
        &read(&app, ".dockerignore"),
        &["target\n", ".git\n", ".wisp/*\n!.wisp/app.css\n"],
    );
    // It only writes; nothing was compiled.
    assert!(!app.join("Cargo.lock").exists());

    // Neither file is overwritten without --force.
    write(&app, "Dockerfile", "mine");
    write(&app, ".dockerignore", "mine too");
    let o = fail(
        &app,
        &["build", "--docker"],
        "Dockerfile and .dockerignore already exist.",
    );
    has(
        &o.err,
        &["Run wisp build --docker --force to replace them."],
    );
    assert_eq!(read(&app, "Dockerfile"), "mine");
    assert_eq!(read(&app, ".dockerignore"), "mine too");

    fs::remove_file(app.join(".dockerignore")).unwrap();
    let o = fail(&app, &["build", "--docker"], "Dockerfile already exists.");
    has(&o.err, &["to replace it."]);
    assert!(
        !app.join(".dockerignore").exists(),
        "nothing is written when one is taken"
    );

    // --force takes both, spelled either way.
    let o = wisp(&app, &["build", "--docker", "--force"]);
    assert!(o.ok, "{}", o.err);
    assert!(read(&app, "Dockerfile").starts_with("# Written by wisp build --docker."));
    write(&app, "Dockerfile", "mine");
    assert!(wisp(&app, &["build", "--force", "--target", "docker"]).ok);
    assert!(read(&app, "Dockerfile").contains("FROM rust:slim"));
    assert!(read(&app, ".dockerignore").contains("dist\n"));
}

#[test]
fn docker_uses_the_package_name_in_cargo_toml() {
    let cwd = Dir::new("docker-name");
    let app = new_app(&cwd, "My App", &["-t", "minimal"]);
    assert!(wisp(&app, &["build", "--docker"]).ok);
    has(
        &read(&app, "Dockerfile"),
        &["target/release/my-app /server"],
    );
    fs::write(app.join("Cargo.toml"), "[workspace]\n").unwrap();
    let o = fail(
        &app,
        &["build", "--docker", "--force"],
        "Cargo.toml has no package name.",
    );
    assert!(o.out.is_empty());
}

#[test]
fn the_css_step_runs_tailwind_or_removes_its_stale_output() {
    let cwd = Dir::new("css");
    // Plain CSS: a stale Tailwind build would shadow it, so it goes.
    let plain = new_app(&cwd, "plain", &["-t", "minimal"]);
    write(&plain, ".wisp/app.css", "stale");
    assert!(wisp(&plain, &["build", "--docker"]).ok);
    assert!(!plain.join(".wisp/app.css").exists());

    let app = new_app(&cwd, "tw", &["-t", "minimal", "--tailwind"]);
    let good = fake_tailwind(&cwd, 0);
    let o = wisp_env(&app, &["build", "--docker"], &[("WISP_TAILWIND", &good)]);
    assert!(o.ok, "{}", o.err);
    assert!(read(&app, ".wisp/app.css").starts_with("fake"));

    // Either quote imports Tailwind.
    let single = new_app(&cwd, "single", &["-t", "minimal"]);
    write(&single, "src/app.css", "@import 'tailwindcss';\n");
    assert!(wisp_env(&single, &["build", "--docker"], &[("WISP_TAILWIND", &good)]).ok);
    assert!(single.join(".wisp/app.css").is_file());

    // A Tailwind that fails stops the build before anything is written.
    let bad_dir = Dir::new("bad-tailwind");
    let bad = fake_tailwind(&bad_dir, 1);
    fs::remove_file(app.join("Dockerfile")).unwrap();
    let o = wisp_env(
        &app,
        &["build", "--docker", "--force"],
        &[("WISP_TAILWIND", &bad)],
    );
    assert!(
        !o.ok && o.err.contains("Tailwind could not build the CSS."),
        "{}",
        o.err
    );
    assert!(!app.join("Dockerfile").exists());

    // And one that is not there says so.
    let o = fail(&app, &["build", "--docker"], "Could not run Tailwind");
    assert!(o.out.is_empty());
}

#[test]
fn release_build_and_static_export() {
    let cwd = Dir::new("release");
    let app = pinned_app(&cwd, "release-site", &["-t", "minimal"]);
    let o = wisp(&app, &["build"]);
    assert!(o.ok, "{}", o.err);
    has(
        &o.out,
        &[
            "Building for release",
            "Built ",
            "MB in ",
            "One file with the CSS and static files inside. Copy it to a server and run it.",
        ],
    );

    let o = wisp(&app, &["build", "--static", "--out=out"]);
    assert!(o.ok, "{}", o.err);
    has(
        &o.out,
        &[
            "Exporting to out",
            " files to out",
            "Serve that folder from any static host",
        ],
    );
    assert_tree(
        &app.join("out"),
        &[
            "404.html",
            "_app/app.css",
            "_app/wisp.js",
            "favicon.svg",
            "index.html",
        ],
    );
    let index = read(&app, "out/index.html");
    has(&index, &["<title>Home</title>", "<h1>Welcome to Wisp</h1>"]);
    assert_eq!(
        read(&app, "out/favicon.svg"),
        read(&app, "static/favicon.svg")
    );
    has(&read(&app, "out/_app/app.css"), &["--accent"]);
    has(&read(&app, "out/404.html"), &["404"]);

    // The folder is `dist` by default; --docker goes with it.
    let o = wisp(&app, &["build", "--static"]);
    assert!(o.ok, "{}", o.err);
    assert!(app.join("dist/index.html").is_file());
    let o = wisp(&app, &["build", "--docker", "--static", "--out", "again"]);
    assert!(o.ok, "{}", o.err);
    assert!(app.join("Dockerfile").is_file() && app.join("again/index.html").is_file());
    assert!(wisp(&app, &["build", "-t", "static", "-o", "third"]).ok);
    assert!(app.join("third/index.html").is_file());

    // What needs a server or a value is said, and left out; the rest is written.
    write(
        &app,
        "src/routes/ping/+server.rs",
        "fn get() -> &'static str {\n    \"pong\"\n}\n",
    );
    write(
        &app,
        "src/routes/form/+page.wisp",
        "---\n#[action]\nfn add() {}\n---\n<form action=\"?/add\"><button>Add</button></form>\n",
    );
    write(&app, "src/routes/[slug]/+page.wisp", "<h1>{slug}</h1>\n");
    let o = wisp(&app, &["build", "--static", "--out", "warned"]);
    assert!(o.ok, "{}", o.err);
    has(
        &o.out,
        &[
            "/ping has a +server.rs, which needs a server, so it is not exported",
            "/form has actions, which need a server: its forms will not work",
            "/[slug]",
        ],
    );
    assert!(app.join("warned/form/index.html").is_file());
    assert!(!app.join("warned/ping").exists() && !app.join("warned/[slug]").exists());

    // Browser modules come with what they import; with --sourcemap, with
    // their maps, and without it, with no line naming one.
    write(
        &app,
        "src/lib/twice.js",
        "export const twice = (n) => n * 2\n",
    );
    write(
        &app,
        "src/routes/count/+page.wisp",
        "<p>{:n}</p>\n<script>\n  import { twice } from '$lib/twice.js'\n  let n = twice(2)\n</script>\n",
    );
    for (out, maps) in [("plain", false), ("mapped", true)] {
        let mut args = vec!["build", "--static", "--out", out];
        if maps {
            args.push("--sourcemap");
        }
        let o = wisp(&app, &args);
        assert!(o.ok, "{}", o.err);
        let files = tree(&app.join(out));
        let js: Vec<&String> = files.iter().filter(|f| f.ends_with(".js")).collect();
        assert!(js.iter().any(|f| f.contains("twice")), "{files:?}");
        for f in js {
            let text = read(&app.join(out), f);
            let named = text.contains("//# sourceMappingURL=");
            let map = files.contains(&format!("{f}.map"));
            assert_eq!(named, map, "{out}/{f}");
            assert!(
                !maps || f.ends_with("wisp.js") || f.ends_with("live.js") || map,
                "{out}/{f}"
            );
        }
        assert_eq!(files.iter().any(|f| f.ends_with(".map")), maps, "{files:?}");
    }

    // A folder that cannot be written is a failed export.
    write(&app, "blocked", "a file, not a folder");
    let o = fail(
        &app,
        &["build", "--static", "--out", "blocked"],
        "The export failed.",
    );
    assert!(o.err.contains("The app's message is above."));
}

#[test]
fn cargo_that_cannot_build_fails_the_build() {
    let cwd = Dir::new("no-crates");
    let app = new_app(&cwd, "site", &["-t", "minimal"]);
    // Offline with nothing downloaded: there is nothing to compile against.
    let home = Dir::new("empty-cargo-home");
    let o = wisp_env(&app, &["build"], &[("CARGO_HOME", &home)]);
    assert!(!o.ok && o.err.contains("The build failed."), "{}", o.err);
    assert!(
        o.out.contains("Building for release") && !o.out.contains("Built "),
        "{}",
        o.out
    );
}

#[test]
fn tailwind_is_taken_from_the_home_folder_when_it_is_there() {
    let cwd = Dir::new("tailwind-home");
    let app = new_app(&cwd, "site", &["-t", "minimal", "--tailwind"]);
    // The pinned version's name, as css.rs has it.
    let css = include_str!("../../src/css.rs");
    let version = css
        .split("TAILWIND_VERSION: &str = \"")
        .nth(1)
        .and_then(|s| s.split('"').next())
        .unwrap();
    // Any program will do for one that is not Tailwind: this one fails when
    // asked for what Tailwind is, which says it was the one run.
    let home = Dir::new("home");
    let bin = home.join(".wisp/bin");
    fs::create_dir_all(&bin).unwrap();
    let name = format!("tailwindcss-{version}{}", std::env::consts::EXE_SUFFIX);
    fs::copy(std::env::current_exe().unwrap(), bin.join(name)).unwrap();

    let mut cmd = command(&app);
    cmd.env_remove("WISP_TAILWIND")
        .env("HOME", &*home)
        .env("USERPROFILE", &*home)
        .args(["build", "--docker"]);
    let out = cmd.output().unwrap();
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        !out.status.success() && err.contains("Tailwind could not build the CSS."),
        "{err}"
    );
    assert!(!String::from_utf8_lossy(&out.stdout).contains("Downloading Tailwind"));
    assert!(!app.join("Dockerfile").exists());
}

#[test]
fn a_client_is_written_for_the_endpoints() {
    let cwd = Dir::new("client");
    let api = new_app(&cwd, "api", &["--api"]);
    let o = wisp(&api, &["build", "--client", "ts"]);
    assert!(o.ok, "{}", o.err);
    has(
        &o.out,
        &["Wrote client.ts", "import { client } from './client'"],
    );
    let ts = read(&api, "client.ts");
    has(
        &ts,
        &["Generated by Wisp", "export interface Note", "client("],
    );

    // --out names the file, and its folders are made; both spellings work.
    let o = wisp(&api, &["build", "--client=ts", "--out", "web/lib/api.ts"]);
    assert!(o.ok, "{}", o.err);
    has(&o.out, &["Wrote web/lib/api.ts", "from './api'"]);
    assert_eq!(read(&api, "web/lib/api.ts"), ts);

    // Nothing to write a client of.
    let site = new_app(&cwd, "site", &["-t", "minimal"]);
    fail(
        &site,
        &["build", "--client", "ts"],
        "The app has no endpoints",
    );
    assert!(!site.join("client.ts").exists());
}

#[test]
fn an_invalid_app_stops_before_cargo_runs() {
    let cwd = Dir::new("invalid");
    let app = new_app(&cwd, "site", &["-t", "minimal"]);
    write(&app, "src/routes/+page.wisp", "<h1>Hi</h1>\n<p>{oops</p>\n");
    for args in [
        &["build"][..],
        &["build", "--static"],
        &["build", "--target", "node"],
        &["build", "--docker", "--static"],
    ] {
        fail(&app, args, "src/routes/+page.wisp:2:4: unclosed {");
    }
    assert!(!app.join("Cargo.lock").exists() && !app.join("dist").exists());
}

#[test]
fn compile_errors_are_told_against_the_template() {
    let cwd = Dir::new("compile-errors");
    let app = pinned_app(&cwd, "errors-site", &["-t", "minimal"]);
    write(
        &app,
        "src/routes/+page.wisp",
        "<h1>Hi</h1>\n<p>{nope}</p>\n",
    );
    let o = fail(&app, &["build", "--target", "node"], "The build failed.");
    has(
        &o.err,
        &["cannot find value `nope`", "--> src/routes/+page.wisp:2:"],
    );
    assert!(!o.err.contains("wisp.rs") && !app.join("dist").exists());

    write(&app, "src/routes/+page.wisp", "<h1>Hi</h1>\n");
    write(
        &app,
        "src/hooks.rs",
        "fn init() {\n    let x: u32 = \"a\";\n}\n",
    );
    let o = fail(&app, &["build", "--target", "node"], "The build failed.");
    has(&o.err, &["mismatched types", "--> src/hooks.rs:2:"]);
    assert!(!app.join("dist").exists());
}

#[test]
fn each_host_gets_its_folder() {
    let cwd = Dir::new("hosts");
    let app = pinned_app(&cwd, "hosts-site", &["-t", "minimal"]);
    let build = |args: &[&str]| {
        let mut all = vec!["build"];
        all.extend_from_slice(args);
        let o = wisp(&app, &all);
        assert!(o.ok, "{all:?}: {}", o.err);
        assert!(
            o.out
                .contains("Set WISP_SECRET there if the app signs cookies.")
        );
        o.out
    };
    let wasm = |rel: &str| {
        let bytes = fs::read(app.join(rel)).unwrap_or_else(|e| panic!("{rel}: {e}"));
        assert!(bytes.starts_with(b"\0asm"), "{rel}");
    };
    let favicon = read(&app, "static/favicon.svg");

    let o = build(&["--target", "cloudflare"]);
    has(
        &o,
        &[
            "Building for cloudflare (WebAssembly)",
            "Wrote dist/cloudflare for cloudflare",
            "npx wrangler deploy",
        ],
    );
    let out = app.join("dist/cloudflare");
    assert_tree(
        &out,
        &[
            "app.wasm",
            "bridge.mjs",
            "public/favicon.svg",
            "worker.js",
            "wrangler.toml",
        ],
    );
    wasm("dist/cloudflare/app.wasm");
    assert_eq!(
        read(&out, "wrangler.toml"),
        "name = \"hosts-site\"\nmain = \"worker.js\"\ncompatibility_date = \"2025-09-01\"\n\n# Served before the worker runs.\n[assets]\ndirectory = \"public\"\n"
    );
    assert_eq!(read(&out, "public/favicon.svg"), favicon);
    has(&read(&out, "worker.js"), &["from './bridge.mjs'"]);

    let o = build(&["-t", "deno", "--out", "d"]);
    has(
        &o,
        &["Wrote d for deno", "deployctl deploy --entrypoint main.ts"],
    );
    assert_tree(&app.join("d"), &["app.wasm", "bridge.mjs", "main.ts"]);
    wasm("d/app.wasm");
    has(&read(&app, "d/main.ts"), &["from './bridge.mjs'"]);

    let o = build(&["--target=vercel"]);
    has(&o, &["npx vercel deploy --prebuilt"]);
    let out = app.join("dist/vercel");
    let func = ".vercel/output/functions/index.func";
    assert_tree(
        &out,
        &[
            ".vercel/output/config.json",
            ".vercel/output/static/favicon.svg",
            &format!("{func}/.vc-config.json"),
            &format!("{func}/app.wasm"),
            &format!("{func}/bridge.mjs"),
            &format!("{func}/index.mjs"),
        ],
    );
    wasm(&format!("dist/vercel/{func}/app.wasm"));
    has(
        &read(&out, ".vercel/output/config.json"),
        &["\"version\":3", "\"dest\":\"/index\""],
    );
    has(
        &read(&out, &format!("{func}/.vc-config.json")),
        &["nodejs22.x", "\"handler\":\"index.mjs\""],
    );
    assert_eq!(read(&out, ".vercel/output/static/favicon.svg"), favicon);

    let o = build(&["--target=netlify", "-o", "n"]);
    has(&o, &["npx netlify deploy --prod"]);
    let out = app.join("n");
    assert_tree(
        &out,
        &[
            "functions/app.wasm.js",
            "functions/bridge.mjs",
            "functions/wisp.mjs",
            "netlify.toml",
            "public/favicon.svg",
        ],
    );
    has(
        &read(&out, "netlify.toml"),
        &["publish = \"public\"", "directory = \"functions\""],
    );
    // The wasm rides along as base64, whose start is `\0asm`'s.
    has(
        &read(&out, "functions/app.wasm.js"),
        &["export default \"AGFzbQ"],
    );
    assert_eq!(read(&out, "public/favicon.svg"), favicon);

    let o = build(&["--target", "node", "--out=node-out"]);
    has(
        &o,
        &[
            "Wrote node-out for node",
            "npm start runs it; hosts/ says how each host takes it",
        ],
    );
    let out = app.join("node-out");
    let hosts =
        ["amplify", "azure", "firebase", "stormkit", "zeabur"].map(|h| format!("hosts/{h}.md"));
    let mut want = vec!["app.wasm", "bridge.mjs", "package.json", "server.mjs"];
    want.extend(hosts.iter().map(String::as_str));
    assert_tree(&out, &want);
    wasm("node-out/app.wasm");
    let package = read(&out, "package.json");
    has(
        &package,
        &[
            "\"name\": \"hosts-site\"",
            "\"type\": \"module\"",
            "\"start\": \"node server.mjs\"",
        ],
    );
    has(&read(&out, "server.mjs"), &["from './bridge.mjs'"]);
    has(&read(&out, "hosts/firebase.md"), &["# Firebase"]);

    // Without a static folder there is nothing for the CDN to serve.
    fs::remove_dir_all(app.join("static")).unwrap();
    build(&["--target", "cloudflare", "--out", "c2"]);
    assert_tree(
        &app.join("c2"),
        &["app.wasm", "bridge.mjs", "worker.js", "wrangler.toml"],
    );
    assert!(!read(&app, "c2/wrangler.toml").contains("[assets]"));
    build(&["--target", "vercel", "--out", "v2"]);
    assert!(app.join("v2/.vercel/output/static").is_dir());
    build(&["--target", "netlify", "--out", "n2"]);
    assert!(app.join("n2/public").is_dir());
}

#[test]
fn the_icon_is_sized_for_the_manifest() {
    let cwd = Dir::new("icon");
    let app = pinned_app(&cwd, "pwa", &["-t", "minimal"]);
    let mut png = b"\x89PNG\r\n\x1a\n\0\0\0\x0dIHDR".to_vec();
    png.extend(600u32.to_be_bytes());
    png.extend(600u32.to_be_bytes());
    png.extend([8, 6, 0, 0, 0]);
    fs::write(app.join("static/icon.png"), &png).unwrap();
    write(&app, "src/manifest.json", "{\"name\": \"Pics\"}");
    let tool = fake_cwebp(&cwd);
    let o = wisp_env(
        &app,
        &["build", "--static", "--out=out"],
        &[("WISP_CWEBP", &tool)],
    );
    assert!(o.ok, "{}", o.err);
    has(&o.out, &["Encoding 2 WebP images into .wisp/img"]);
    let h = wisp_build::image::hash(&png);
    has(
        &read(&app, "out/manifest.webmanifest"),
        &[&format!(
            "\"icons\":[{{\"src\":\"/icon.png\",\"sizes\":\"600x600\",\"type\":\"image/png\"}},\
             {{\"src\":\"/_app/img/{h}-192.webp\",\"sizes\":\"192x192\",\"type\":\"image/webp\"}},\
             {{\"src\":\"/_app/img/{h}-512.webp\",\"sizes\":\"512x512\",\"type\":\"image/webp\"}}]"
        )],
    );
    let files = tree(&app.join("out"));
    for f in [
        format!("_app/img/{h}-192.webp"),
        format!("_app/img/{h}-512.webp"),
    ] {
        assert!(files.contains(&f), "{f} in {files:?}");
    }
    has(
        &read(&app, "out/index.html"),
        &["<link rel=\"manifest\" href=\"/manifest.webmanifest\">"],
    );
}

/// A stand-in for cwebp that writes its `-o` file.
fn fake_cwebp(dir: &Path) -> std::path::PathBuf {
    let (name, script) = if cfg!(windows) {
        (
            "cwebp.cmd",
            "@echo off\r\n:next\r\nif \"%~1\"==\"-o\" (echo webp>\"%~2\"& exit /b 0)\r\nshift\r\nif not \"%~1\"==\"\" goto next\r\nexit /b 1\r\n",
        )
    } else {
        (
            "cwebp.sh",
            "#!/bin/sh\nwhile [ $# -gt 0 ]; do\n  if [ \"$1\" = -o ]; then echo webp > \"$2\"; exit 0; fi\n  shift\ndone\nexit 1\n",
        )
    };
    let path = dir.join(name);
    fs::write(&path, script).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    }
    path
}

#[test]
fn images_get_webp_widths_and_their_size() {
    use wisp_build::image::hash;
    let cwd = Dir::new("images");
    let app = pinned_app(&cwd, "pics", &["-t", "minimal"]);
    // Headers are all the build reads; the stand-in "encodes" any file.
    let mut png = b"\x89PNG\r\n\x1a\n\0\0\0\x0dIHDR".to_vec();
    png.extend(1500u32.to_be_bytes());
    png.extend(300u32.to_be_bytes());
    png.extend([8, 2, 0, 0, 0]);
    let jpg = [
        0xff, 0xd8, 0xff, 0xc0, 0, 11, 8, 0, 48, 0, 64, 1, 1, 0x11, 0, 0xff, 0xd9,
    ];
    fs::create_dir_all(app.join("src/lib")).unwrap();
    fs::write(app.join("src/lib/cat.png"), &png).unwrap();
    fs::write(app.join("static/dog.jpg"), jpg).unwrap();
    write(
        &app,
        "src/routes/pics/+page.wisp",
        "<img src=\"$lib/cat.png\" alt=\"Cat\">\n<img src=\"/dog.jpg\" alt=\"Dog\" loading=\"eager\">\n",
    );
    let tool = fake_cwebp(&cwd);
    let build = |tool: &Path| {
        wisp_env(
            &app,
            &["build", "--static", "--out=out"],
            &[("WISP_CWEBP", tool)],
        )
    };
    let o = build(&tool);
    assert!(o.ok, "{}", o.err);
    has(&o.out, &["Encoding 4 WebP images into .wisp/img"]);
    let (cat, dog) = (hash(&png), hash(&jpg));
    has(
        &read(&app, "out/pics/index.html"),
        &[
            &format!(
                "<img src=\"/_app/img/{cat}.png\" alt=\"Cat\" width=\"1500\" height=\"300\" \
                 srcset=\"/_app/img/{cat}-640.webp 640w, /_app/img/{cat}-1280.webp 1280w, /_app/img/{cat}-1500.webp 1500w\" \
                 sizes=\"100vw\" loading=\"lazy\" decoding=\"async\">"
            ),
            &format!(
                "<img src=\"/dog.jpg\" alt=\"Dog\" loading=\"eager\" width=\"64\" height=\"48\" \
                 srcset=\"/_app/img/{dog}-64.webp 64w\" sizes=\"100vw\" decoding=\"async\">"
            ),
        ],
    );
    let files = tree(&app.join("out"));
    for f in [
        format!("_app/img/{cat}.png"),
        format!("_app/img/{cat}-640.webp"),
        format!("_app/img/{cat}-1500.webp"),
        format!("_app/img/{dog}-64.webp"),
        "dog.jpg".into(),
    ] {
        assert!(files.contains(&f), "{f} in {files:?}");
    }
    assert_eq!(tree(&app.join(".wisp/img")).len(), 4);

    // A second build encodes nothing.
    let o = build(&tool);
    assert!(o.ok, "{}", o.err);
    assert!(!o.out.contains("Encoding"), "{}", o.out);

    // Without cwebp the build warns, and serves the original, sized.
    fs::remove_dir_all(app.join(".wisp/img")).unwrap();
    let o = build(&cwd.join("no-such-cwebp"));
    assert!(o.ok, "{}", o.err);
    has(&o.out, &["4 of 4 WebP images could not be written"]);
    let page = read(&app, "out/pics/index.html");
    has(
        &page,
        &[&format!(
            "<img src=\"/_app/img/{cat}.png\" alt=\"Cat\" width=\"1500\" height=\"300\" loading=\"lazy\" decoding=\"async\">"
        )],
    );
    assert!(!page.contains("srcset"), "{page}");
}
