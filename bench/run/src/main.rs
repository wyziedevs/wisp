//! Wisp against other web stacks on this machine, on Linux or Windows.
//!
//! Builds every server in release mode, then runs each one alone on half of
//! the CPU cores with the load generator on the other half, so the two never
//! compete for a core. Prints throughput, latency, CPU time per request,
//! response size, peak memory and time to first response, then a Markdown
//! table of it all, fastest first on each path, with Wisp's rank.
//!
//!   cargo run -r -p bench-run -- [-c 64] [-d 10] [-w 5] [--rounds 1]
//!       [--group fast|popular|all] [--only wisp,actix] [--paths fortunes]
//!       [--no-build] [--csv FILE] [--extra NAME=COMMAND]...
//!
//! `--group` picks the ten fastest frameworks (TechEmpower's top tier), the
//! ten most popular, or both (the default); Wisp is in each. `--only` and
//! `--paths` narrow that by case-insensitive substring. `--extra
//! NAME=COMMAND` adds a server of your own, measured on `/plaintext` only;
//! it gets `PORT` and `THREADS` like the others. `--rounds` runs every
//! server that many times, taking turns, and reports the mean. `--csv`
//! appends every run to a file. A server whose toolchain is not installed
//! is skipped with a note.

mod sys;

use std::collections::HashMap;
use std::io::Write;
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::time::{Duration, Instant};

#[derive(Clone, Copy, PartialEq)]
enum Bin {
    Wisp,
    Rust,
    Go,
    AspNet,
    Node,
    Bun,
    Java,
    /// `--extra`: `args[0]` is the program.
    Other,
}

/// `--group fast`: TechEmpower's top tier, one or two per language.
const FAST: u8 = 1;
/// `--group popular`: the most used framework of each kind.
const POPULAR: u8 = 2;
/// Runs on Linux only: its Windows I/O is broken (may-minihttp answers a
/// kept-alive connection's first request again), or its processes share
/// the port with SO_REUSEPORT, which Windows lacks.
const LINUX: u8 = 4;

struct Server {
    name: &'static str,
    bin: Bin,
    args: &'static [&'static str],
    /// The variable that sets the server's thread or process count.
    threads: &'static str,
    env: &'static [(&'static str, &'static str)],
    /// Paths it serves besides `/plaintext`, `/fortunes` and `/json`.
    extra: &'static [&'static str],
    tags: u8,
}

const fn server(
    name: &'static str,
    bin: Bin,
    args: &'static [&'static str],
    threads: &'static str,
    tags: u8,
) -> Server {
    Server {
        name,
        bin,
        args,
        threads,
        env: &[],
        extra: &[],
        tags,
    }
}

const SERVERS: &[Server] = &[
    Server {
        env: &[("HOST", "127.0.0.1")],
        // A `#[derive(Rest)]` row, beside `/json`'s hand-written one;
        // `/fortunes` kept for a second (`CACHE`); its table as a page with
        // nothing to compute, baked at build time.
        extra: &["/messages/1", "/fortunes-cached", "/static"],
        ..server("Wisp", Bin::Wisp, &[], "WISP_THREADS", FAST | POPULAR)
    },
    // Popular, then fast; each list is the top ten.
    Server {
        extra: &["/fortunes-blazor"],
        ..server("ASP.NET Core", Bin::AspNet, &[], "", POPULAR)
    },
    server(
        "Actix Web",
        Bin::Rust,
        &["actix"],
        "THREADS",
        FAST | POPULAR,
    ),
    server("Axum", Bin::Rust, &["axum"], "THREADS", POPULAR),
    server("Go net/http", Bin::Go, &["nethttp"], "GOMAXPROCS", POPULAR),
    server("Gin", Bin::Go, &["gin"], "GOMAXPROCS", POPULAR),
    server("Fiber", Bin::Go, &["fiber"], "GOMAXPROCS", POPULAR),
    server(
        "Express",
        Bin::Node,
        &["cluster.mjs", "express/server.mjs"],
        "WORKERS",
        POPULAR,
    ),
    server(
        "Fastify",
        Bin::Node,
        &["cluster.mjs", "fastify/server.mjs"],
        "WORKERS",
        POPULAR,
    ),
    Server {
        env: &[("HOST", "127.0.0.1")],
        ..server(
            "SvelteKit",
            Bin::Node,
            &["cluster.mjs", "sveltekit/build/index.js"],
            "WORKERS",
            POPULAR,
        )
    },
    Server {
        env: &[("HOSTNAME", "127.0.0.1")],
        ..server(
            "Next.js",
            Bin::Node,
            &["cluster.mjs", "nextjs/.next/standalone/nextjs/server.js"],
            "WORKERS",
            POPULAR,
        )
    },
    server("may-minihttp", Bin::Rust, &["may"], "THREADS", FAST | LINUX),
    server("xitca-web", Bin::Rust, &["xitca"], "THREADS", FAST),
    server("ntex", Bin::Rust, &["ntex"], "THREADS", FAST),
    server("hyper", Bin::Rust, &["hyper"], "THREADS", FAST),
    server("fasthttp", Bin::Go, &["fasthttp"], "GOMAXPROCS", FAST),
    server(
        "Vert.x",
        Bin::Java,
        &["-XX:+UseParallelGC", "-jar", "target/bench.jar"],
        "THREADS",
        FAST,
    ),
    server(
        "uWebSockets.js",
        Bin::Node,
        &["cluster.mjs", "uws/server.mjs"],
        "WORKERS",
        FAST | LINUX,
    ),
    server(
        "Bun",
        Bin::Bun,
        &["bun-cluster.js", "bun/server.js"],
        "WORKERS",
        FAST | LINUX,
    ),
    server(
        "Elysia",
        Bin::Bun,
        &["bun-cluster.js", "elysia/server.js"],
        "WORKERS",
        FAST | LINUX,
    ),
];

struct Options {
    connections: usize,
    duration: Duration,
    warmup: Duration,
    rounds: usize,
    group: u8,
    only: Vec<String>,
    paths: Vec<String>,
    build: bool,
    csv: Option<PathBuf>,
    extra: Vec<Server>,
}

/// One server on one path, one round.
struct Row {
    server: &'static str,
    path: &'static str,
    rps: f64,
    p50: u64,
    p99: u64,
    p999: u64,
    cpu_us: f64,
    kernel_us: f64,
    bytes: u64,
    peak_mb: f64,
    start_ms: u64,
    failures: u64,
}

fn main() {
    let opt = options();
    let repo = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("repo directory");
    let bench = repo.join("bench");

    let mut servers: Vec<&Server> = SERVERS
        .iter()
        .filter(|s| s.tags & opt.group != 0 && picked(&opt.only, s.name))
        .collect();
    servers.retain(|s| {
        let ok = cfg!(target_os = "linux") || s.tags & LINUX == 0;
        if !ok {
            println!("skipping {}: runs on Linux only", s.name);
        }
        ok
    });
    servers.retain(|s| {
        let missing: Vec<&str> = toolchain(s.bin)
            .iter()
            .copied()
            .filter(|t| !installed(t))
            .collect();
        if !missing.is_empty() {
            println!(
                "skipping {}: `{}` is not installed",
                s.name,
                missing.join("`, `")
            );
        }
        missing.is_empty()
    });
    if opt.build {
        build(&repo, &bench, &mut servers);
    }
    servers.extend(&opt.extra);
    if servers.is_empty() {
        die("no servers to run");
    }

    let (server_cpus, load_cpus) = sys::split_cpus();
    sys::pin_self(&load_cpus);
    let threads = server_cpus.len().to_string();
    println!(
        "servers on CPUs {}, load on {}; {} connections, {}s warmup, {}s measured, {} round{}",
        sys::list(&server_cpus),
        sys::list(&load_cpus),
        opt.connections,
        opt.warmup.as_secs(),
        opt.duration.as_secs(),
        opt.rounds,
        if opt.rounds == 1 { "" } else { "s" }
    );

    let mut rows = Vec::new();
    for round in 1..=opt.rounds {
        for (i, s) in servers.iter().enumerate() {
            let all: &[&str] = if s.bin == Bin::Other {
                &["/plaintext"]
            } else {
                &["/plaintext", "/fortunes", "/json"]
            };
            let paths: Vec<&'static str> = all
                .iter()
                .chain(s.extra)
                .copied()
                .filter(|p| picked(&opt.paths, p))
                .collect();
            if paths.is_empty() {
                continue;
            }
            let port = 3401 + i as u16;
            let addr: SocketAddr = ([127, 0, 0, 1], port).into();
            // Whatever answers there is not the server we are about to start,
            // and measuring it would print someone else's numbers.
            if !port_free(addr, Duration::from_secs(5)) {
                die(&format!(
                    "port {port} is taken: is another benchmark running? Stop it first."
                ));
            }
            let mut child = match start(s, &repo, &bench, port, &threads, &server_cpus) {
                Ok(child) => child,
                Err(e) => {
                    println!("{}: {e}", s.name);
                    continue;
                }
            };
            match measure(s, &mut child, addr, &paths, &opt, server_cpus.len()) {
                Ok(new) => rows.extend(new),
                Err(e) => println!("{}: {e}", s.name),
            }
            sys::kill(&mut child);
            if opt.rounds > 1 {
                println!("  (round {round} of {})", opt.rounds);
            }
        }
    }

    if let Some(file) = &opt.csv {
        write_csv(file, &rows);
    }
    println!();
    print_table(&rows, opt.connections);
}

fn options() -> Options {
    let mut opt = Options {
        connections: 64,
        duration: Duration::from_secs(10),
        warmup: Duration::from_secs(5),
        rounds: 1,
        group: FAST | POPULAR,
        only: Vec::new(),
        paths: Vec::new(),
        build: true,
        csv: None,
        extra: Vec::new(),
    };
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        let mut value = || {
            args.next()
                .unwrap_or_else(|| die(&format!("{a} needs a value")))
        };
        let number = |v: String| -> u64 {
            v.parse()
                .unwrap_or_else(|_| die(&format!("{a}: not a number: {v}")))
        };
        let list = |v: String| -> Vec<String> {
            v.split(',')
                .map(|s| s.trim().to_ascii_lowercase())
                .collect()
        };
        match a.as_str() {
            "-c" => opt.connections = number(value()) as usize,
            "-d" => opt.duration = Duration::from_secs(number(value()).max(1)),
            "-w" => opt.warmup = Duration::from_secs(number(value())),
            "--rounds" => opt.rounds = number(value()).max(1) as usize,
            "--group" => {
                opt.group = match value().as_str() {
                    "fast" => FAST,
                    "popular" => POPULAR,
                    "all" => FAST | POPULAR,
                    other => die(&format!("--group: fast, popular or all, not {other}")),
                }
            }
            "--only" => opt.only = list(value()),
            "--paths" => opt.paths = list(value()),
            "--no-build" => opt.build = false,
            "--csv" => opt.csv = Some(value().into()),
            "--extra" => {
                let v = value();
                let (name, command) = v
                    .split_once('=')
                    .unwrap_or_else(|| die("--extra takes NAME=COMMAND"));
                // Lives as long as the run, like the built-in table.
                let args: Vec<&'static str> = command
                    .split_whitespace()
                    .map(|a| &*a.to_string().leak())
                    .collect();
                if args.is_empty() {
                    die("--extra takes NAME=COMMAND");
                }
                opt.extra.push(Server {
                    name: name.to_string().leak(),
                    bin: Bin::Other,
                    args: args.leak(),
                    threads: "THREADS",
                    env: &[],
                    extra: &[],
                    tags: FAST | POPULAR,
                });
            }
            "-h" | "--help" => {
                println!(
                    "usage: bench-run [-c 64] [-d 10] [-w 5] [--rounds 1] [--group fast|popular|all] [--only wisp,actix] [--paths fortunes] [--no-build] [--csv FILE] [--extra NAME=COMMAND]..."
                );
                std::process::exit(0);
            }
            _ => die(&format!("unknown argument {a}; see --help")),
        }
    }
    opt
}

/// Whether `name` is picked by a list of substrings; an empty list picks all.
fn picked(list: &[String], name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    list.is_empty() || list.iter().any(|want| name.contains(want.as_str()))
}

/// The commands a server needs to build and run.
fn toolchain(bin: Bin) -> &'static [&'static str] {
    match bin {
        Bin::Wisp | Bin::Rust => &["cargo"],
        Bin::Go => &["go"],
        Bin::AspNet => &["dotnet"],
        Bin::Node => &["node", "npm"],
        Bin::Bun => &["bun"],
        Bin::Java => &["java", "mvn"],
        Bin::Other => &[],
    }
}

/// `tool` as `Command` finds it: npm and mvn are batch files on Windows.
fn program(tool: &str) -> String {
    if cfg!(windows) && matches!(tool, "npm" | "mvn") {
        format!("{tool}.cmd")
    } else {
        tool.to_string()
    }
}

fn installed(tool: &str) -> bool {
    let arg = if tool == "go" { "version" } else { "--version" };
    Command::new(program(tool))
        .arg(arg)
        .output()
        .is_ok_and(|o| o.status.success())
}

/// Where cargo puts a workspace's release binaries.
fn release_dir(workspace: &Path) -> PathBuf {
    let target = std::env::var_os("CARGO_TARGET_DIR")
        .map_or_else(|| workspace.join("target"), PathBuf::from);
    target.join("release")
}

fn exe(name: &str) -> String {
    format!("{name}{}", std::env::consts::EXE_SUFFIX)
}

/// What builds a server: servers sharing a binary or a directory share it.
fn unit(s: &Server) -> &'static str {
    match s.bin {
        Bin::Wisp => "wisp",
        Bin::Rust => "rust",
        Bin::Go => "go",
        Bin::AspNet => "aspnet",
        Bin::Java => "java",
        // The app's directory: `cluster.mjs fastify/server.mjs` is fastify.
        Bin::Node | Bin::Bun => s.args[1].split('/').next().unwrap_or(""),
        Bin::Other => "",
    }
}

/// Builds every server in `servers`, then drops those whose build failed.
fn build(repo: &Path, bench: &Path, servers: &mut Vec<&Server>) {
    let mut units: Vec<&str> = Vec::new();
    for s in servers.iter() {
        if !units.contains(&unit(s)) {
            units.push(unit(s));
        }
    }
    let mut failed: Vec<&str> = Vec::new();
    for u in units {
        let node = bench.join("node").join(u);
        let ok = match u {
            "wisp" => run(Command::new("cargo")
                .args(["build", "--release", "-q", "-p", "wisp-bench"])
                .current_dir(repo)),
            "rust" => run(Command::new("cargo")
                .args(["build", "--release", "-q"])
                .current_dir(bench.join("rust"))),
            "aspnet" => run(Command::new("dotnet")
                .args([
                    "publish", "-c", "Release", "-o", "out", "--nologo", "-v", "q",
                ])
                .current_dir(bench.join("aspnet"))),
            "go" => run(Command::new("go")
                .args([
                    "build",
                    "-mod=mod",
                    "-o",
                    &format!("out/{}", exe("bench-go")),
                    ".",
                ])
                .current_dir(bench.join("go"))),
            "java" => run(Command::new(program("mvn"))
                .args(["-q", "-B", "package"])
                .current_dir(bench.join("java"))),
            // Bun.serve needs nothing; bun install is quick when all is there.
            "bun" => true,
            "elysia" => run(Command::new("bun")
                .args(["install", "--silent"])
                .current_dir(&node)),
            _ => {
                let npm = |args: &[&str]| {
                    run(Command::new(program("npm"))
                        .args(args)
                        .env("NEXT_TELEMETRY_DISABLED", "1")
                        .current_dir(&node))
                };
                let built = matches!(u, "sveltekit" | "nextjs");
                (installed_since(&node)
                    || npm(&["install", "--no-audit", "--no-fund", "--loglevel=error"]))
                    && (!built || npm(&["run", "build", "--silent"]))
            }
        };
        if !ok {
            failed.push(u);
        }
    }
    servers.retain(|s| {
        let ok = !failed.contains(&unit(s));
        if !ok {
            println!("skipping {}: its build failed", s.name);
        }
        ok
    });
}

/// Whether `dir` has an npm install newer than its package.json. npm
/// writes `.package-lock.json` last, so a half-done install is redone.
fn installed_since(dir: &Path) -> bool {
    let time = |p: PathBuf| std::fs::metadata(p).and_then(|m| m.modified()).ok();
    match (
        time(dir.join("node_modules/.package-lock.json")),
        time(dir.join("package.json")),
    ) {
        (Some(installed), Some(changed)) => installed >= changed,
        _ => false,
    }
}

/// Runs a build step, echoing it; false if it failed.
fn run(cmd: &mut Command) -> bool {
    println!(
        "› {} {}",
        cmd.get_program().to_string_lossy(),
        cmd.get_args()
            .map(|a| a.to_string_lossy())
            .collect::<Vec<_>>()
            .join(" ")
    );
    match cmd.status() {
        Ok(s) if s.success() => true,
        Ok(s) => {
            println!("  build failed ({s})");
            false
        }
        Err(e) => {
            println!("  could not run the build: {e}");
            false
        }
    }
}

fn start(
    s: &Server,
    repo: &Path,
    bench: &Path,
    port: u16,
    threads: &str,
    cpus: &[usize],
) -> Result<Child, String> {
    let (program, dir) = match s.bin {
        Bin::Wisp => (
            release_dir(repo).join(exe("wisp-bench")),
            repo.to_path_buf(),
        ),
        Bin::Rust => (
            release_dir(&bench.join("rust")).join(exe("bench-rust")),
            bench.join("rust"),
        ),
        Bin::Go => (bench.join("go/out").join(exe("bench-go")), bench.join("go")),
        Bin::AspNet => (
            bench.join("aspnet/out").join(exe("Bench")),
            bench.join("aspnet/out"),
        ),
        Bin::Node => (PathBuf::from("node"), bench.join("node")),
        Bin::Bun => (PathBuf::from("bun"), bench.join("node")),
        Bin::Java => (PathBuf::from("java"), bench.join("java")),
        Bin::Other => (PathBuf::from(s.args[0]), repo.to_path_buf()),
    };
    let args = if s.bin == Bin::Other {
        &s.args[1..]
    } else {
        s.args
    };
    let mut cmd = Command::new(&program);
    cmd.args(args)
        .current_dir(dir)
        .env("PORT", port.to_string())
        .env("ASPNETCORE_URLS", format!("http://127.0.0.1:{port}"));
    if !s.threads.is_empty() {
        cmd.env(s.threads, threads);
    }
    cmd.envs(s.env.iter().copied());
    sys::spawn_pinned(&mut cmd, cpus)
        .map_err(|e| format!("cannot start {}: {e}", program.display()))
}

/// Waits for the server, checks what it sends, then loads each path.
fn measure(
    s: &Server,
    child: &mut Child,
    addr: SocketAddr,
    paths: &[&'static str],
    opt: &Options,
    cpus: usize,
) -> Result<Vec<Row>, String> {
    let start_ms = wait_ready(child, addr)?;
    for path in paths.iter().filter(|p| p.starts_with("/fortunes")) {
        check_fortunes(addr, path)?;
    }
    let tree = sys::tree(child.id());
    println!(
        "\n== {}  (first response after {start_ms} ms, {} process{})",
        s.name,
        tree.len(),
        if tree.len() == 1 { "" } else { "es" }
    );

    let mut rows = Vec::new();
    for &path in paths {
        let tree = sys::tree(child.id());
        let before = sys::cpu_times(&tree);
        let wall = Instant::now();
        let r = wisp_load::run(addr, path, opt.connections, opt.warmup, opt.duration);
        let after = sys::cpu_times(&tree);
        let wall = wall.elapsed().as_secs_f64();
        if let Ok(Some(status)) = child.try_wait() {
            return Err(format!("exited during {path} ({status})"));
        }
        // CPU time over the whole run (warmup included) per second of it, so
        // the cores kept busy; divided by the measured rate, CPU per request.
        let (mut total, mut kernel) = (0.0, 0.0);
        for (pid, (t, k)) in &after {
            if let Some((t0, k0)) = before.get(pid) {
                total += t - t0;
                kernel += k - k0;
            }
        }
        let rps = r.rps();
        let (cores, kernel_cores) = (total / wall, kernel / wall);
        let peak: u64 = tree.iter().map(|&pid| sys::peak_memory(pid)).sum();
        let row = Row {
            server: s.name,
            path,
            rps,
            p50: r.latency.percentile(0.50),
            p99: r.latency.percentile(0.99),
            p999: r.latency.percentile(0.999),
            cpu_us: cores * 1e6 / rps,
            kernel_us: kernel_cores * 1e6 / rps,
            bytes: r.bytes.checked_div(r.ok).unwrap_or(0),
            peak_mb: peak as f64 / (1024.0 * 1024.0),
            start_ms,
            failures: r.non_2xx + r.errors,
        };
        println!(
            "  {path:<17} {:>9.0} req/s  p50 {}  p99 {}  p99.9 {}  {:.1} of {} cores, {:.2} µs CPU per request ({:.2} kernel){}",
            row.rps,
            wisp_load::ms(row.p50),
            wisp_load::ms(row.p99),
            wisp_load::ms(row.p999),
            cores,
            cpus,
            row.cpu_us,
            row.kernel_us,
            if row.failures > 0 {
                format!(", {} failures", row.failures)
            } else {
                String::new()
            }
        );
        rows.push(row);
    }
    Ok(rows)
}

/// Whether nothing accepts connections on `addr`, waiting up to `wait` for
/// a server just killed to let go of it.
fn port_free(addr: SocketAddr, wait: Duration) -> bool {
    let t = Instant::now();
    while TcpStream::connect_timeout(&addr, Duration::from_millis(100)).is_ok() {
        if t.elapsed() > wait {
            return false;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    true
}

/// Milliseconds from launch until the first successful response.
fn wait_ready(child: &mut Child, addr: SocketAddr) -> Result<u64, String> {
    let t = Instant::now();
    loop {
        if let Ok(Some(status)) = child.try_wait() {
            return Err(format!("exited before answering ({status})"));
        }
        if t.elapsed() > Duration::from_secs(30) {
            return Err("did not answer within 30 s".into());
        }
        if let Some((200, _)) = wisp_load::get(addr, "/plaintext", Duration::from_millis(250)) {
            return Ok(t.elapsed().as_millis() as u64);
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// Every server must send the same rows in the same order, escaped, with
/// the non-ASCII row as is.
fn check_fortunes(addr: SocketAddr, path: &str) -> Result<(), String> {
    let Some((200, body)) = wisp_load::get(addr, path, Duration::from_secs(5)) else {
        return Err(format!("{path} did not answer 200"));
    };
    let html = String::from_utf8_lossy(&body);
    let ids: Vec<&str> = html
        .split("<td>")
        .skip(1)
        .filter_map(|cell| cell.split_once("</td>"))
        .map(|(id, _)| id)
        .filter(|c| c.bytes().all(|b| b.is_ascii_digit()))
        .collect();
    if ids.join(",") != "11,4,5,2,8,0,3,7,10,6,9,1,12" {
        return Err(format!("{path} sent rows {}", ids.join(",")));
    }
    if html.contains("<script>alert") {
        return Err(format!("{path} did not escape the script row"));
    }
    if !html.contains("フレームワークのベンチマーク") {
        return Err(format!("{path} did not send the non-ASCII row as is"));
    }
    Ok(())
}

fn write_csv(file: &Path, rows: &[Row]) {
    let new = !file.exists();
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(file)
        .unwrap_or_else(|e| die(&format!("{}: {e}", file.display())));
    let mut text = String::new();
    if new {
        text.push_str("server,path,req/s,p50 us,p99 us,p99.9 us,cpu us,kernel us,bytes,peak MB,start ms,failures\n");
    }
    for r in rows {
        text.push_str(&format!(
            "{},{},{:.0},{},{},{},{:.2},{:.2},{},{:.1},{},{}\n",
            r.server,
            r.path,
            r.rps,
            r.p50,
            r.p99,
            r.p999,
            r.cpu_us,
            r.kernel_us,
            r.bytes,
            r.peak_mb,
            r.start_ms,
            r.failures
        ));
    }
    f.write_all(text.as_bytes())
        .unwrap_or_else(|e| die(&format!("{}: {e}", file.display())));
}

/// The mean of every round per server and path, as a Markdown table,
/// fastest first on each path, then where Wisp ranks on each.
fn print_table(rows: &[Row], connections: usize) {
    let mut order: Vec<(&str, &str)> = Vec::new();
    let mut groups: HashMap<(&str, &str), Vec<&Row>> = HashMap::new();
    for r in rows {
        let key = (r.server, r.path);
        if !groups.contains_key(&key) {
            order.push(key);
        }
        groups.entry(key).or_default().push(r);
    }
    let rounds = groups.values().map(Vec::len).max().unwrap_or(1);
    let rps = |key: &(&str, &str)| {
        let g = &groups[key];
        g.iter().map(|r| r.rps).sum::<f64>() / g.len() as f64
    };
    // Paths in the order they ran; on each, the fastest first.
    let mut paths: Vec<&str> = Vec::new();
    for key in &order {
        if !paths.contains(&key.1) {
            paths.push(key.1);
        }
    }
    let at = |path: &str| paths.iter().position(|p| *p == path);
    order.sort_by(|a, b| at(a.1).cmp(&at(b.1)).then(rps(b).total_cmp(&rps(a))));

    let mut table = vec![vec![
        "Path".to_string(),
        "#".into(),
        "Server".into(),
        "req/s".into(),
        "p50".into(),
        "p99".into(),
        "p99.9".into(),
        "CPU µs/req".into(),
        "Kernel µs".into(),
        "Bytes".into(),
        "Peak MB".into(),
        "First response".into(),
    ]];
    if rounds > 1 {
        table[0].push("CPU min..max".into());
    }
    let mut rank = 0;
    for (i, key) in order.iter().enumerate() {
        rank = if i > 0 && order[i - 1].1 == key.1 {
            rank + 1
        } else {
            1
        };
        let g = &groups[key];
        let mean = |f: fn(&Row) -> f64| g.iter().map(|r| f(r)).sum::<f64>() / g.len() as f64;
        let mut line = vec![
            key.1.to_string(),
            rank.to_string(),
            if key.0 == "Wisp" {
                "**Wisp**".to_string()
            } else {
                key.0.to_string()
            },
            thousands(mean(|r| r.rps) as u64),
            wisp_load::ms(mean(|r| r.p50 as f64) as u64),
            wisp_load::ms(mean(|r| r.p99 as f64) as u64),
            wisp_load::ms(mean(|r| r.p999 as f64) as u64),
            format!("{:.1}", mean(|r| r.cpu_us)),
            format!("{:.1}", mean(|r| r.kernel_us)),
            format!("{:.0}", mean(|r| r.bytes as f64)),
            format!("{:.0}", mean(|r| r.peak_mb)),
            format!("{:.0} ms", mean(|r| r.start_ms as f64)),
        ];
        if rounds > 1 {
            let lo = g.iter().map(|r| r.cpu_us).fold(f64::MAX, f64::min);
            let hi = g.iter().map(|r| r.cpu_us).fold(0.0, f64::max);
            line.push(format!("{lo:.1}..{hi:.1}"));
        }
        table.push(line);
    }

    // Text columns left, numbers right.
    let left = |c: usize| c == 0 || c == 2;
    let width: Vec<usize> = (0..table[0].len())
        .map(|c| {
            table
                .iter()
                .map(|l| l[c].chars().count())
                .max()
                .unwrap_or(0)
        })
        .collect();
    for (i, line) in table.iter().enumerate() {
        let cells: Vec<String> = line
            .iter()
            .enumerate()
            .map(|(c, cell)| {
                if left(c) {
                    format!("{cell:<w$}", w = width[c])
                } else {
                    format!("{cell:>w$}", w = width[c])
                }
            })
            .collect();
        println!("| {} |", cells.join(" | "));
        if i == 0 {
            let rules: Vec<String> = width
                .iter()
                .enumerate()
                .map(|(c, &w)| {
                    if left(c) {
                        "-".repeat(w + 2)
                    } else {
                        format!("{}:", "-".repeat(w + 1))
                    }
                })
                .collect();
            println!("|{}|", rules.join("|"));
        }
    }
    println!(
        "\n{connections} connections, mean of {rounds} round{}.\n",
        if rounds == 1 { "" } else { "s" }
    );

    for path in &paths {
        let ranked: Vec<(&str, f64)> = order
            .iter()
            .filter(|k| k.1 == *path)
            .map(|k| (k.0, rps(k)))
            .collect();
        let n = ranked.len();
        let (best, best_rps) = ranked[0];
        let best_text = thousands(best_rps as u64);
        match ranked.iter().position(|r| r.0 == "Wisp") {
            // One server's own path, like ASP.NET's /fortunes-blazor.
            None if n == 1 => {}
            None => println!(
                "rank {path}: Wisp not measured; fastest of {n} is {best} ({best_text} req/s)"
            ),
            Some(0) if n == 1 => println!("rank {path}: Wisp alone, {best_text} req/s"),
            Some(0) => {
                let (next, next_rps) = ranked[1];
                println!(
                    "rank {path}: Wisp 1st of {n}, {best_text} req/s, {:.0}% ahead of {next} ({} req/s)",
                    (best_rps / next_rps - 1.0) * 100.0,
                    thousands(next_rps as u64)
                );
            }
            Some(i) => println!(
                "rank {path}: Wisp {} of {n}, {} req/s, {:.0}% of {best} ({best_text} req/s)",
                ordinal(i + 1),
                thousands(ranked[i].1 as u64),
                ranked[i].1 / best_rps * 100.0
            ),
        }
    }
}

fn ordinal(n: usize) -> String {
    let suffix = match (n % 10, n % 100) {
        (_, 11..=13) => "th",
        (1, _) => "st",
        (2, _) => "nd",
        (3, _) => "rd",
        _ => "th",
    };
    format!("{n}{suffix}")
}

fn thousands(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

fn die(msg: &str) -> ! {
    eprintln!("bench-run: {msg}");
    std::process::exit(2);
}
