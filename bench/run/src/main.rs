//! Wisp against other web stacks on this machine, on Linux or Windows.
//!
//! Builds every server in release mode, then runs each one alone on half of
//! the CPU cores with the load generator on the other half, so the two never
//! compete for a core. Prints throughput, latency, CPU time per request,
//! response size, peak memory, time to first response and deploy size, then
//! a Markdown table of it all, fastest first on each path, with Wisp's rank
//! on each.
//!
//!   cargo run -r -p bench-run -- [-c 64] [-d 10] [-w 5] [--rounds 1]
//!       [--pipeline 1] [--group fast,popular,top|all] [--only wisp,actix]
//!       [--paths fortunes] [--no-build] [--csv FILE] [--extra NAME=COMMAND]...
//!       [--suite benchmarker|real] [--think 1.0] [--max-users 50000]
//!       [--slo-ms 100] [--tests users,churn,slow,wait,...] [--soak 60]
//!
//! `--group` picks the ten fastest frameworks (TechEmpower's top tier), the
//! ten most popular, the-benchmarker's top ten, or a list of them (all by
//! default); Wisp is in each. `--only` and `--paths` narrow that by
//! case-insensitive substring. `--pipeline N` sends N requests back to back
//! on each connection before reading their N responses, as TechEmpower's
//! plaintext does with 16 (browsers do not pipeline, so it is off by
//! default). `--extra NAME=COMMAND` adds a server of your own, measured on
//! `/plaintext` only; it gets `PORT` and `THREADS` like the others.
//! `--rounds` runs every server that many times, taking turns, and reports
//! the mean. `--csv` appends every run to a file. A server whose toolchain
//! is not installed is skipped with a note.
//!
//! `--suite benchmarker` measures what the-benchmarker's web-frameworks
//! board does instead (github.com/the-benchmarker/web-frameworks): `GET /`,
//! `GET /user/0` and `POST /user`, each closed loop for 15 s at 64, 256 and
//! 512 connections (`-c` takes a list here) after one 5 s warmup of `GET /`
//! at 50, with zrk and their flags where zrk is installed, else wisp-load
//! sending zrk's request. Servers are ranked as their results page ranks
//! them: by the mean of the three routes' rates at 64 connections. Wisp runs
//! on epoll there, as in their Docker containers, whose seccomp profile
//! refuses io_uring (`WISP_IO=uring` in the environment measures io_uring).
//!
//! `--suite real` measures what happens in practice instead (see real.rs):
//! how many users each sending a request every `--think` seconds a server
//! serves within a p99 of `--slo-ms` (up to `--max-users`, `-d` seconds a
//! step) and what twice that does to it, a new connection per request,
//! slowloris clients beside normal ones; then the app's own work (waiting,
//! JSON in, uploads, big JSON out, static files, WebSockets) and robustness
//! (malformed requests, memory over `--soak` seconds, stopping on SIGTERM).
//! `--tests` picks some of them.

mod real;
mod sys;

use std::collections::HashMap;
use std::io::Write;
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
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
    Nim,
    Dart,
    /// `--extra`: `args[0]` is the program.
    Other,
}

/// `--group fast`: TechEmpower's top tier, one or two per language.
const FAST: u8 = 1;
/// `--group popular`: the most used framework of each kind.
const POPULAR: u8 = 2;
/// Runs on Linux only: its Windows I/O is broken (may-minihttp answers a
/// kept-alive connection's first request again), its processes share the
/// port with SO_REUSEPORT, which Windows lacks, or it is written for Linux
/// alone (Caprese, jet_server).
const LINUX: u8 = 4;
/// `--group top`: in the-benchmarker's top ten on its results page (at 64
/// connections, 2026-09-28).
const TOP: u8 = 8;
/// Answers only the-benchmarker's three routes, as its entry there does, so
/// runs only in `--suite benchmarker`.
const THEIRS: u8 = 16;

struct Server {
    name: &'static str,
    bin: Bin,
    args: &'static [&'static str],
    /// The variable that sets the server's thread or process count.
    threads: &'static str,
    env: &'static [(&'static str, &'static str)],
    /// Paths it serves besides `/plaintext`, `/fortunes`, `/json` and `/page`.
    extra: &'static [&'static str],
    /// The port it always listens on; 0 takes the one it is given.
    port: u16,
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
        port: 0,
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
        ..server("Wisp", Bin::Wisp, &[], "WISP_THREADS", FAST | POPULAR | TOP)
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
        // adapter-node refuses bodies over 512 KB by default; /upload
        // takes 8 MiB.
        env: &[("HOST", "127.0.0.1"), ("BODY_SIZE_LIMIT", "8M")],
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
    server(
        "may-minihttp",
        Bin::Rust,
        &["may"],
        "THREADS",
        FAST | TOP | LINUX,
    ),
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
        FAST | TOP | LINUX,
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
    // The rest of the-benchmarker's top ten: their entries, answering only
    // their routes. Caprese's port is a constant of its build, and it starts
    // a thread per CPU the machine has.
    Server {
        port: 3000,
        ..server("Caprese", Bin::Nim, &[], "", TOP | THEIRS | LINUX)
    },
    server(
        "fulmine.js",
        Bin::Node,
        &["fulmine/server.mjs"],
        "WORKERS",
        TOP | THEIRS | LINUX,
    ),
    server(
        "jet_server",
        Bin::Dart,
        &[],
        "THREADS",
        TOP | THEIRS | LINUX,
    ),
    server("Ohkami", Bin::Rust, &["ohkami"], "THREADS", TOP | THEIRS),
    server(
        "MoroJS engine",
        Bin::Node,
        &["cluster.mjs", "morojs/server.mjs"],
        "WORKERS",
        TOP | THEIRS | LINUX,
    ),
    server(
        "ActiveJ",
        Bin::Java,
        &["-XX:+UseParallelGC", "-jar", "target/bench.jar", "activej"],
        "THREADS",
        TOP | THEIRS,
    ),
];

/// the-benchmarker's routes, as "METHOD /path", and the body its contract
/// wants back from each (github.com/the-benchmarker/web-frameworks, .env and
/// .spec/route_spec.rb).
const ROUTES: [(&str, &str); 3] = [("GET /", ""), ("GET /user/0", "0"), ("POST /user", "")];

struct Options {
    /// One level, or `--suite benchmarker`'s list of them.
    connections: Vec<usize>,
    duration: Duration,
    warmup: Duration,
    rounds: usize,
    /// Requests in flight per connection; 1 is closed loop.
    pipeline: usize,
    group: u8,
    only: Vec<String>,
    paths: Vec<String>,
    build: bool,
    csv: Option<PathBuf>,
    extra: Vec<Server>,
    /// What runs; `think` to `soak` are `Suite::Real`'s.
    suite: Suite,
    think: Duration,
    max_users: usize,
    slo_ms: u64,
    /// Which of `real::TESTS` to run, and how long the soak lasts.
    tests: Vec<String>,
    soak: Duration,
    /// In the suite, zrk's threads (one per load CPU, as their harness gives
    /// it) when zrk is installed; else wisp-load sends the load.
    zrk: Option<usize>,
}

#[derive(Clone, Copy, PartialEq)]
enum Suite {
    /// Our routes, closed loop: each server's peak.
    Peak,
    /// `--suite benchmarker`: the-benchmarker's routes and load, not ours.
    Benchmarker,
    /// `--suite real`: see real.rs.
    Real,
}

/// One server on one path (a route, in the suite) at one level, one round.
struct Row {
    server: &'static str,
    path: &'static str,
    connections: usize,
    rps: f64,
    p50: u64,
    p99: u64,
    p999: u64,
    cpu_us: f64,
    kernel_us: f64,
    bytes: u64,
    peak_mb: f64,
    start_ms: u64,
    /// What it takes to deploy, where the server has a size of its own.
    size: Option<u64>,
    failures: u64,
}

fn main() {
    let mut opt = options();
    let (server_cpus, load_cpus) = sys::split_cpus();
    if opt.suite == Suite::Real {
        // A user is a connection here and one in the server.
        if let Some(limit) = sys::raise_open_files() {
            let need = 2 * opt.max_users as u64 + 1000;
            if limit < need {
                let max = (limit.saturating_sub(1000) / 2).max(1) as usize;
                println!(
                    "warning: {limit} open files at most (`ulimit -n`), {need} needed for {} users and twice that: capping users at {}",
                    thousands(opt.max_users as u64),
                    thousands(max as u64)
                );
                opt.max_users = max;
            }
        }
        sys::fine_timers();
    }
    opt.zrk = (opt.suite == Suite::Benchmarker && installed("zrk")).then_some(load_cpus.len());
    let repo = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("repo directory");
    let bench = repo.join("bench");

    let mut servers: Vec<&Server> = SERVERS
        .iter()
        .filter(|s| {
            s.tags & opt.group != 0
                && (opt.suite == Suite::Benchmarker || s.tags & THEIRS == 0)
                && picked(&opt.only, s.name)
        })
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

    sys::pin_self(&load_cpus);
    let threads = server_cpus.len().to_string();
    let rounds = format!(
        "{} round{}",
        opt.rounds,
        if opt.rounds == 1 { "" } else { "s" }
    );
    if opt.suite == Suite::Real {
        real::announce_suite(&opt, &server_cpus, &load_cpus);
    } else if opt.suite == Suite::Benchmarker {
        let levels: Vec<String> = opt.connections.iter().map(usize::to_string).collect();
        println!(
            "the-benchmarker suite: servers on CPUs {}, load on {} ({}); {} connections; {}s warmup of GET / at 50, then {}s a route; {rounds}",
            sys::list(&server_cpus),
            sys::list(&load_cpus),
            match opt.zrk {
                Some(t) => format!("zrk --closed, {t} threads, as theirs"),
                None => "wisp-load sending zrk's request: zrk is not installed".into(),
            },
            levels.join(", "),
            opt.warmup.as_secs(),
            opt.duration.as_secs(),
        );
    } else {
        println!(
            "servers on CPUs {}, load on {}; {} connections, {}, {}s warmup, {}s measured, {rounds}",
            sys::list(&server_cpus),
            sys::list(&load_cpus),
            opt.connections[0],
            mode(opt.pipeline),
            opt.warmup.as_secs(),
            opt.duration.as_secs(),
        );
    }

    let mut rows = Vec::new();
    let mut real_rows = Vec::new();
    for round in 1..=opt.rounds {
        for (i, s) in servers.iter().enumerate() {
            let all: &[&str] = if opt.suite == Suite::Benchmarker {
                &ROUTES.map(|(route, _)| route)
            } else if s.bin == Bin::Other {
                &["/plaintext"]
            } else {
                &["/plaintext", "/fortunes", "/json", "/page"]
            };
            let extra = if opt.suite == Suite::Benchmarker {
                &[][..]
            } else {
                s.extra
            };
            let paths: Vec<&'static str> = all
                .iter()
                .chain(extra)
                .copied()
                .filter(|p| picked(&opt.paths, p))
                .collect();
            if paths.is_empty() {
                continue;
            }
            let port = if s.port > 0 { s.port } else { 3401 + i as u16 };
            let addr: SocketAddr = ([127, 0, 0, 1], port).into();
            // Whatever answers there is not the server we are about to start,
            // and measuring it would print someone else's numbers.
            if !port_free(addr, Duration::from_secs(5)) {
                die(&format!(
                    "port {port} is taken: is another benchmark running? Stop it first."
                ));
            }
            let mut child = match start(
                s,
                &repo,
                &bench,
                port,
                &threads,
                &server_cpus,
                opt.suite == Suite::Benchmarker,
            ) {
                Ok(child) => child,
                Err(e) => {
                    println!("{}: {e}", s.name);
                    continue;
                }
            };
            let size = deploy_size(s, &repo, &bench);
            let cpus = server_cpus.len();
            if opt.suite == Suite::Real {
                match real::measure(s, &mut child, addr, &opt, cpus, load_cpus.len()) {
                    Ok(row) => real_rows.push(row),
                    Err(e) => println!("{}: {e}", s.name),
                }
                sys::kill(&mut child);
                continue;
            }
            let measured = if opt.suite == Suite::Benchmarker {
                suite(s, &mut child, addr, &paths, &opt, cpus, size)
            } else {
                measure(s, &mut child, addr, &paths, &opt, cpus, size)
            };
            match measured {
                Ok(new) => rows.extend(new),
                Err(e) => println!("{}: {e}", s.name),
            }
            sys::kill(&mut child);
            if opt.rounds > 1 {
                println!("  (round {round} of {})", opt.rounds);
            }
        }
    }

    if opt.suite == Suite::Real {
        if let Some(file) = &opt.csv {
            real::write_csv(file, &real_rows);
        }
        println!();
        real::print(&real_rows, &opt);
        return;
    }
    if let Some(file) = &opt.csv {
        write_csv(file, &rows, opt.pipeline);
    }
    println!();
    if opt.suite == Suite::Benchmarker {
        print_suite(&rows, &opt.connections, server_cpus.len());
    } else {
        print_table(&rows, opt.connections[0], opt.pipeline);
    }
}

fn options() -> Options {
    let mut opt = Options {
        connections: Vec::new(),
        duration: Duration::ZERO,
        warmup: Duration::from_secs(5),
        rounds: 1,
        pipeline: 1,
        group: FAST | POPULAR | TOP,
        only: Vec::new(),
        paths: Vec::new(),
        build: true,
        csv: None,
        extra: Vec::new(),
        suite: Suite::Peak,
        think: Duration::from_secs(1),
        max_users: 50_000,
        slo_ms: 100,
        tests: real::TESTS.iter().map(|t| t.to_string()).collect(),
        soak: Duration::from_secs(60),
        zrk: None,
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
            "-c" => {
                opt.connections = list(value())
                    .into_iter()
                    .map(|c| number(c).max(1) as usize)
                    .collect()
            }
            "-d" => opt.duration = Duration::from_secs(number(value()).max(1)),
            "-w" => opt.warmup = Duration::from_secs(number(value())),
            "--rounds" => opt.rounds = number(value()).max(1) as usize,
            "--pipeline" => opt.pipeline = number(value()).max(1) as usize,
            "--group" => {
                opt.group = list(value())
                    .iter()
                    .map(|g| match g.as_str() {
                        "fast" => FAST,
                        "popular" => POPULAR,
                        "top" => TOP,
                        "all" => FAST | POPULAR | TOP,
                        other => die(&format!("--group: fast, popular, top or all, not {other}")),
                    })
                    .fold(0, |all, g| all | g)
            }
            "--suite" => match value().as_str() {
                "benchmarker" => opt.suite = Suite::Benchmarker,
                "real" => opt.suite = Suite::Real,
                other => die(&format!("--suite: benchmarker or real, not {other}")),
            },
            "--think" => {
                let v = value();
                let secs: f64 = v
                    .parse()
                    .unwrap_or_else(|_| die(&format!("--think: not a number: {v}")));
                opt.think = Duration::from_secs_f64(secs.max(0.001));
            }
            "--max-users" => opt.max_users = number(value()).max(1) as usize,
            "--slo-ms" => opt.slo_ms = number(value()).max(1),
            "--tests" => {
                opt.tests = list(value());
                if let Some(t) = opt
                    .tests
                    .iter()
                    .find(|t| !real::TESTS.contains(&t.as_str()))
                {
                    die(&format!("--tests: {}, not {t}", real::TESTS.join(",")));
                }
            }
            "--soak" => opt.soak = Duration::from_secs(number(value())),
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
                opt.extra.push(server(
                    name.to_string().leak(),
                    Bin::Other,
                    args.leak(),
                    "THREADS",
                    FAST | POPULAR,
                ));
            }
            "-h" | "--help" => {
                println!(
                    "usage: bench-run [-c 64] [-d 10] [-w 5] [--rounds 1] [--pipeline 1] [--group fast,popular,top|all] [--only wisp,actix] [--paths fortunes] [--no-build] [--csv FILE] [--extra NAME=COMMAND]... [--suite benchmarker|real] [--think 1.0] [--max-users 50000] [--slo-ms 100] [--tests users,churn,...] [--soak 60]"
                );
                std::process::exit(0);
            }
            _ => die(&format!("unknown argument {a}; see --help")),
        }
    }
    // the-benchmarker's .env: 15 s a route at 64, 256 and 512 connections.
    if opt.connections.is_empty() {
        opt.connections = if opt.suite == Suite::Benchmarker {
            vec![64, 256, 512]
        } else {
            vec![64]
        };
    }
    if opt.duration.is_zero() {
        opt.duration = Duration::from_secs(if opt.suite == Suite::Benchmarker {
            15
        } else {
            10
        });
    }
    if opt.suite == Suite::Benchmarker && opt.pipeline > 1 {
        die("--suite benchmarker loads closed loop; --pipeline does not apply");
    }
    if opt.suite == Suite::Real && (opt.pipeline > 1 || opt.rounds > 1) {
        die("--suite real runs once, closed loop or open; --pipeline and --rounds do not apply");
    }
    if opt.suite != Suite::Benchmarker && opt.connections.len() > 1 {
        die("-c takes one level, or a list with --suite benchmarker");
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
        Bin::Nim => &["nim", "nimble"],
        Bin::Dart => &["dart"],
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
        Bin::Nim => "nim",
        Bin::Dart => "dart",
        // The app's directory: `cluster.mjs fastify/server.mjs` is fastify.
        Bin::Node | Bin::Bun => s
            .args
            .iter()
            .find_map(|a| a.split_once('/'))
            .map_or("", |(dir, _)| dir),
        Bin::Other => "",
    }
}

/// What it takes to deploy a server: its binary or app directory, without
/// the runtime (.NET, the JVM, Node, Bun and Dart's are installed apart).
/// Servers that share one binary or jar with others have no size of their
/// own.
fn deploy_size(s: &Server, repo: &Path, bench: &Path) -> Option<u64> {
    let node = bench.join("node").join(unit(s));
    let parts = match (s.bin, unit(s)) {
        (Bin::Wisp, _) => vec![release_dir(repo).join(exe("wisp-bench"))],
        (Bin::AspNet, _) => vec![bench.join("aspnet/out")],
        (Bin::Nim | Bin::Dart, dir) => vec![bench.join(dir).join(exe("server"))],
        (Bin::Node | Bin::Bun, "sveltekit") => vec![node.join("build")],
        (Bin::Node | Bin::Bun, "nextjs") => {
            vec![node.join(".next/standalone"), node.join(".next/static")]
        }
        (Bin::Bun, "bun") => vec![node.join("server.js")],
        (Bin::Node | Bin::Bun, _) => vec![node.join("node_modules")],
        (Bin::Rust | Bin::Go | Bin::Java | Bin::Other, _) => return None,
    };
    parts.iter().map(|p| bytes_in(p)).sum()
}

/// Bytes in a file, or in every file under a directory.
fn bytes_in(path: &Path) -> Option<u64> {
    let meta = std::fs::symlink_metadata(path).ok()?;
    if !meta.is_dir() {
        return Some(meta.len());
    }
    std::fs::read_dir(path)
        .ok()?
        .map(|e| bytes_in(&e.ok()?.path()))
        .sum()
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
            "wisp" => {
                app_js(bench, "app/static/static")
                    && run(Command::new("cargo")
                        .args(["build", "--release", "-q", "-p", "wisp-bench"])
                        .current_dir(repo))
            }
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
            // As the-benchmarker's Dockerfiles build them.
            "nim" => {
                let nim = bench.join("nim");
                run(Command::new("nimble")
                    .args(["install", "-y", "--depsOnly"])
                    .env("NOSSL", "1")
                    .current_dir(&nim))
                    && run(Command::new("nim")
                        .args([
                            "c",
                            "-d:release",
                            "--opt:speed",
                            "--assertions:off",
                            "--warnings:off",
                            "--hints:off",
                            "--passC:-flto",
                            "--passL:-flto",
                            "server.nim",
                        ])
                        .current_dir(&nim))
            }
            "dart" => {
                let dart = bench.join("dart");
                run(Command::new("dart").args(["pub", "get"]).current_dir(&dart))
                    && run(Command::new("dart")
                        .args(["compile", "exe", "server.dart", "-o", &exe("server")])
                        .current_dir(&dart))
            }
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
                let public = if u == "nextjs" { "public" } else { "static" };
                (!built || app_js(bench, &format!("node/{u}/{public}/static")))
                    && (installed_since(&node)
                    || npm(&["install", "--no-audit", "--no-fund", "--loglevel=error"]))
                    && (!built || npm(&["run", "build", "--silent"]))
                    // The standalone build serves `public/` only from beside
                    // its server.js, where Next.js's docs say to copy it.
                    && (u != "nextjs"
                        || copy_dir(
                            &node.join("public"),
                            &node.join(".next/standalone/nextjs/public"),
                        )
                        .map_err(|e| println!("  copying public/: {e}"))
                        .is_ok())
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

/// Puts `static/app.js` in `bench/dir`, for a server that serves only its
/// own folder; rewritten only when it differs, so the build that embeds it
/// does not rerun for nothing.
fn app_js(bench: &Path, dir: &str) -> bool {
    let (from, dir) = (bench.join("static/app.js"), bench.join(dir));
    let to = dir.join("app.js");
    let put = || -> std::io::Result<()> {
        let js = std::fs::read(&from)?;
        if std::fs::read(&to).ok().as_ref() != Some(&js) {
            std::fs::create_dir_all(&dir)?;
            std::fs::write(&to, js)?;
        }
        Ok(())
    };
    put()
        .map_err(|e| println!("  copying {}: {e}", to.display()))
        .is_ok()
}

/// Copies every file under `from` to the same place under `to`.
fn copy_dir(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let target = to.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir(&entry.path(), &target)?;
        } else {
            std::fs::copy(entry.path(), target)?;
        }
    }
    Ok(())
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

/// Starts a server pinned to `cpus`. In the suite Wisp runs on epoll, as in
/// the-benchmarker's containers, unless `WISP_IO` says otherwise.
fn start(
    s: &Server,
    repo: &Path,
    bench: &Path,
    port: u16,
    threads: &str,
    cpus: &[usize],
    suite: bool,
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
        Bin::Nim | Bin::Dart => {
            let dir = bench.join(unit(s));
            (dir.join(exe("server")), dir)
        }
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
    if suite && s.bin == Bin::Wisp && std::env::var_os("WISP_IO").is_none() {
        cmd.env("WISP_IO", "epoll");
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
    size: Option<u64>,
) -> Result<Vec<Row>, String> {
    let start_ms = wait_ready(child, addr, &wisp_load::get_request(addr, "/plaintext"))?;
    for path in paths.iter().filter(|p| p.starts_with("/fortunes")) {
        check_fortunes(addr, path)?;
    }
    if paths.contains(&"/page") {
        check_page(addr, "/page")?;
    }
    announce(s, start_ms, sys::tree(child.id()).len());

    let mut rows = Vec::new();
    for &path in paths {
        let tree = sys::tree(child.id());
        let before = sys::cpu_times(&tree);
        let wall = Instant::now();
        let r = wisp_load::run(
            addr,
            &wisp_load::get_request(addr, path),
            opt.connections[0],
            opt.pipeline,
            opt.warmup,
            opt.duration,
        );
        let (total, kernel) = cpu_used(&before, &sys::cpu_times(&tree));
        let wall = wall.elapsed().as_secs_f64();
        if let Ok(Some(status)) = child.try_wait() {
            return Err(format!("exited during {path} ({status})"));
        }
        // CPU time over the whole run (warmup included) per second of it, so
        // the cores kept busy; divided by the measured rate, CPU per request.
        let rps = r.rps();
        let (cores, kernel_cores) = (total / wall, kernel / wall);
        let peak: u64 = tree.iter().map(|&pid| sys::peak_memory(pid)).sum();
        let row = Row {
            server: s.name,
            path,
            connections: opt.connections[0],
            rps,
            p50: r.latency.percentile(0.50),
            p99: r.latency.percentile(0.99),
            p999: r.latency.percentile(0.999),
            cpu_us: cores * 1e6 / rps,
            kernel_us: kernel_cores * 1e6 / rps,
            bytes: r.bytes.checked_div(r.ok).unwrap_or(0),
            peak_mb: peak as f64 / (1024.0 * 1024.0),
            start_ms,
            size,
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

/// the-benchmarker's measurement of a server: their contract checked (a
/// 2xx on each route, empty but for the id), one warmup of `GET /` at 50
/// connections, then each route closed loop at each level, with no warmup
/// of its own, as their Makefile's `warmup` and `collect` targets run zrk.
fn suite(
    s: &Server,
    child: &mut Child,
    addr: SocketAddr,
    routes: &[&'static str],
    opt: &Options,
    cpus: usize,
    size: Option<u64>,
) -> Result<Vec<Row>, String> {
    let zrk = opt.zrk;
    let start_ms = wait_ready(child, addr, &zrk_request(addr, "GET /"))?;
    for (route, want) in ROUTES {
        match wisp_load::send(addr, &zrk_request(addr, route), Duration::from_secs(5)) {
            Some((200..=299, body)) if body == want.as_bytes() => {}
            Some((status, body)) => {
                return Err(format!(
                    "{route} answered {status} {:?}, not a 2xx with {want:?}",
                    String::from_utf8_lossy(&body)
                ));
            }
            None => return Err(format!("{route} did not answer")),
        }
    }
    let tree = sys::tree(child.id());
    announce(s, start_ms, tree.len());
    if !opt.warmup.is_zero() {
        load(addr, "GET /", 50, opt.warmup, zrk)?;
    }

    let mut rows = Vec::new();
    for &connections in &opt.connections {
        let mut sum = 0.0;
        for &route in routes {
            let before = sys::cpu_times(&tree);
            let wall = Instant::now();
            let r = load(addr, route, connections, opt.duration, zrk)?;
            let (total, kernel) = cpu_used(&before, &sys::cpu_times(&tree));
            let wall = wall.elapsed().as_secs_f64();
            if let Ok(Some(status)) = child.try_wait() {
                return Err(format!("exited during {route} ({status})"));
            }
            let peak: u64 = tree.iter().map(|&pid| sys::peak_memory(pid)).sum();
            let row = Row {
                server: s.name,
                path: route,
                connections,
                rps: r.rps,
                p50: r.p50,
                p99: r.p99,
                p999: r.p999,
                cpu_us: total / wall * 1e6 / r.rps,
                kernel_us: kernel / wall * 1e6 / r.rps,
                bytes: r.bytes,
                peak_mb: peak as f64 / (1024.0 * 1024.0),
                start_ms,
                size,
                failures: r.failures,
            };
            println!(
                "  c={connections:<4} {route:<12} {:>9.0} req/s  p50 {}  p99 {}  {:.1} of {cpus} cores, {:.2} µs CPU per request{}",
                row.rps,
                wisp_load::ms(row.p50),
                wisp_load::ms(row.p99),
                total / wall,
                row.cpu_us,
                if row.failures > 0 {
                    format!(", {} failures", row.failures)
                } else {
                    String::new()
                }
            );
            sum += row.rps;
            rows.push(row);
        }
        println!(
            "  c={connections:<4} {:<12} {:>9.0} req/s",
            "mean",
            sum / routes.len() as f64
        );
    }
    Ok(rows)
}

/// What one closed-loop run measured.
struct Load {
    rps: f64,
    p50: u64,
    p99: u64,
    p999: u64,
    /// Per response, head included.
    bytes: u64,
    failures: u64,
}

/// `route` loaded closed loop at `connections` for `time`: by zrk with the
/// flags the-benchmarker's harness passes it (`zrk` is its threads), or by
/// wisp-load sending the request zrk sends.
fn load(
    addr: SocketAddr,
    route: &str,
    connections: usize,
    time: Duration,
    zrk: Option<usize>,
) -> Result<Load, String> {
    let request = zrk_request(addr, route);
    let Some(threads) = zrk else {
        let r = wisp_load::run(addr, &request, connections, 1, Duration::ZERO, time);
        return Ok(Load {
            rps: r.rps(),
            p50: r.latency.percentile(0.50),
            p99: r.latency.percentile(0.99),
            p999: r.latency.percentile(0.999),
            bytes: r.bytes.checked_div(r.ok).unwrap_or(0),
            failures: r.non_2xx + r.errors,
        });
    };
    let (method, path) = route.split_once(' ').unwrap_or(("GET", route));
    let report = std::env::temp_dir().join(format!("bench-run-zrk-{}.json", std::process::id()));
    let _ = std::fs::remove_file(&report);
    let status = Command::new("zrk")
        .args(["--plain", "--closed", "-t", &threads.to_string()])
        .args(["-c", &connections.to_string()])
        .args(["-d", &format!("{}s", time.as_secs())])
        .args([
            "-m",
            method,
            "--timeout",
            "8s",
            "--format",
            "json",
            "--output",
        ])
        .arg(&report)
        .arg(format!("http://{addr}{path}"))
        .stdout(Stdio::null())
        .status()
        .map_err(|e| format!("cannot run zrk: {e}"))?;
    let json = std::fs::read_to_string(&report)
        .map_err(|e| format!("zrk ({status}) wrote no report: {e}"))?;
    let n = |key: &str| number(&json, key).ok_or_else(|| format!("zrk's report has no {key}"));
    let mut failures = 0.0;
    for key in ["connect", "read", "write", "timeout", "non_2xx_3xx"] {
        failures += n(key)?;
    }
    Ok(Load {
        rps: n("achieved_rate")?,
        p50: n("p50")? as u64,
        p99: n("p99")? as u64,
        p999: n("p99_9")? as u64,
        bytes: (n("bytes")? / n("requests")?.max(1.0)) as u64,
        failures: failures as u64,
    })
}

/// The request zrk sends for `route` ("METHOD /path"): its user agent,
/// keep-alive said aloud, and a zero length on a POST with no body.
fn zrk_request(addr: SocketAddr, route: &str) -> String {
    let (method, path) = route.split_once(' ').unwrap_or(("GET", route));
    let length = if method == "POST" {
        "Content-Length: 0\r\n"
    } else {
        ""
    };
    format!(
        "{method} {path} HTTP/1.1\r\nHost: {addr}\r\nUser-Agent: zrk\r\nConnection: keep-alive\r\n{length}\r\n"
    )
}

/// The number after `"key": ` in zrk's JSON report, whose keys are unique.
fn number(json: &str, key: &str) -> Option<f64> {
    let rest = &json[json.find(&format!("\"{key}\": "))? + key.len() + 4..];
    rest[..rest.find([',', ' ', '\n', '}'])?].parse().ok()
}

/// The server's line before its results.
fn announce(s: &Server, start_ms: u64, processes: usize) {
    println!(
        "\n== {}  (first response after {start_ms} ms, {processes} process{})",
        s.name,
        if processes == 1 { "" } else { "es" }
    );
}

/// CPU seconds used between two readings of the same processes: in all,
/// and in the kernel.
fn cpu_used(before: &HashMap<u32, (f64, f64)>, after: &HashMap<u32, (f64, f64)>) -> (f64, f64) {
    let (mut total, mut kernel) = (0.0, 0.0);
    for (pid, (t, k)) in after {
        if let Some((t0, k0)) = before.get(pid) {
            total += t - t0;
            kernel += k - k0;
        }
    }
    (total, kernel)
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

/// Milliseconds from launch until `request` is first answered with a 2xx.
fn wait_ready(child: &mut Child, addr: SocketAddr, request: &str) -> Result<u64, String> {
    let t = Instant::now();
    let mut last = None;
    loop {
        if let Ok(Some(status)) = child.try_wait() {
            return Err(format!("exited before answering ({status})"));
        }
        if t.elapsed() > Duration::from_secs(30) {
            return Err(match last {
                Some(status) => format!("answered {status}, not a 2xx, for 30 s"),
                None => "did not answer within 30 s".into(),
            });
        }
        match wisp_load::send(addr, request, Duration::from_millis(250)) {
            Some((200..=299, _)) => return Ok(t.elapsed().as_millis() as u64),
            Some((status, _)) => last = Some(status),
            None => {}
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

/// `/page` is the layout, the h1, 50 rows and the form, however each server's
/// template spaces them: whitespace, comments and how `"` is escaped in text
/// do not matter. Each row must have the class, id, number and (escaped)
/// name of the same formula the servers use.
fn check_page(addr: SocketAddr, path: &str) -> Result<(), String> {
    let Some((200, body)) = wisp_load::get(addr, path, Duration::from_secs(5)) else {
        return Err(format!("{path} did not answer 200"));
    };
    let html = without_comments(&String::from_utf8_lossy(&body));
    for part in [
        "<title>Roster</title>",
        "<nav",
        "<h1>Roster</h1>",
        "<footer",
        "<form",
        "method=\"post\"",
        "name=\"email\"",
        "<button",
    ] {
        if !html.contains(part) {
            return Err(format!("{path} has no {part}"));
        }
    }
    let rows: Vec<&str> = html
        .split("<tr")
        .filter(|row| row.contains("<td>"))
        .collect();
    if rows.len() != 50 {
        return Err(format!("{path} sent {} rows, not 50", rows.len()));
    }
    for (row, id) in rows.iter().zip(1usize..) {
        let class = row
            .split_once("class=\"")
            .and_then(|(_, rest)| rest.split_once('"'))
            .map(|(class, _)| class);
        let cells: Vec<&str> = row
            .split("<td>")
            .skip(1)
            .filter_map(|cell| cell.split_once("</td>"))
            .map(|(cell, _)| cell.trim())
            .collect();
        let (want_id, want_score) = (id.to_string(), (id * 37 % 101).to_string());
        let ok = class == Some(if id % 3 != 0 { "on" } else { "off" })
            && matches!(cells[..], [i, name, score]
                if i == want_id && score == want_score
                    && unescape(name).as_deref() == Some(PAGE_NAMES[id % 5]));
        if !ok {
            return Err(format!("{path} row {id}: class {class:?}, cells {cells:?}"));
        }
    }
    Ok(())
}

/// The names `/page`'s rows cycle through, by `id % 5`.
const PAGE_NAMES: [&str; 5] = [
    "Ada <&\"",
    "Alan <&\"",
    "Grace <&\"",
    "Linus <&\"",
    "Edsger <&\"",
];

/// `html` without its comments (Svelte and React leave markers).
fn without_comments(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut rest = html;
    while let Some((before, after)) = rest.split_once("<!--") {
        out.push_str(before);
        rest = after.split_once("-->").map_or("", |(_, tail)| tail);
    }
    out + rest
}

/// `text` with its entities decoded (`&lt;`, `&#60;` and `&#x3c;` alike);
/// `None` if a `<` or a `&` that starts no entity is left in it, as
/// unescaped text would.
fn unescape(text: &str) -> Option<String> {
    if text.contains('<') {
        return None;
    }
    let mut out = String::new();
    let mut rest = text;
    while let Some((before, after)) = rest.split_once('&') {
        out += before;
        let (entity, tail) = after.split_once(';')?;
        out.push(match entity {
            "amp" => '&',
            "lt" => '<',
            "gt" => '>',
            "quot" => '"',
            _ => {
                let code = entity.strip_prefix('#')?;
                let code = match code.strip_prefix(['x', 'X']) {
                    Some(hex) => u32::from_str_radix(hex, 16),
                    None => code.parse(),
                };
                char::from_u32(code.ok()?)?
            }
        });
        rest = tail;
    }
    Some(out + rest)
}

/// How the load is sent, for headings: results with and without pipelining
/// are not comparable.
fn mode(pipeline: usize) -> String {
    if pipeline > 1 {
        format!("pipelined ×{pipeline}")
    } else {
        "closed loop".to_string()
    }
}

fn write_csv(file: &Path, rows: &[Row], pipeline: usize) {
    let new = !file.exists();
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(file)
        .unwrap_or_else(|e| die(&format!("{}: {e}", file.display())));
    let mut text = String::new();
    if new {
        text.push_str("server,path,req/s,p50 us,p99 us,p99.9 us,cpu us,kernel us,bytes,peak MB,start ms,failures,pipeline,size bytes,connections\n");
    }
    for r in rows {
        text.push_str(&format!(
            "{},{},{:.0},{},{},{},{:.2},{:.2},{},{:.1},{},{},{},{},{}\n",
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
            r.failures,
            pipeline,
            r.size.map_or(String::new(), |b| b.to_string()),
            r.connections
        ));
    }
    f.write_all(text.as_bytes())
        .unwrap_or_else(|e| die(&format!("{}: {e}", file.display())));
}

/// The mean of every round per server and path, as a Markdown table,
/// fastest first on each path, then where Wisp ranks on each path (in
/// requests per second and CPU per request) and on what is a server's own:
/// peak memory, time to first response and deploy size.
fn print_table(rows: &[Row], connections: usize, pipeline: usize) {
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
    let mean = |key: &(&str, &str), f: fn(&Row) -> f64| {
        let g = &groups[key];
        g.iter().map(|r| f(r)).sum::<f64>() / g.len() as f64
    };
    let rps = |key: &(&str, &str)| mean(key, |r| r.rps);
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
        "Deploy size".into(),
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
        let mut line = vec![
            key.1.to_string(),
            rank.to_string(),
            if key.0 == "Wisp" {
                "**Wisp**".to_string()
            } else {
                key.0.to_string()
            },
            thousands(rps(key) as u64),
            wisp_load::ms(mean(key, |r| r.p50 as f64) as u64),
            wisp_load::ms(mean(key, |r| r.p99 as f64) as u64),
            wisp_load::ms(mean(key, |r| r.p999 as f64) as u64),
            format!("{:.1}", mean(key, |r| r.cpu_us)),
            format!("{:.1}", mean(key, |r| r.kernel_us)),
            format!("{:.0}", mean(key, |r| r.bytes as f64)),
            format!("{:.0}", mean(key, |r| r.peak_mb)),
            format!("{:.0} ms", mean(key, |r| r.start_ms as f64)),
            g[0].size.map_or("-".to_string(), size_text),
        ];
        if rounds > 1 {
            let lo = g.iter().map(|r| r.cpu_us).fold(f64::MAX, f64::min);
            let hi = g.iter().map(|r| r.cpu_us).fold(0.0, f64::max);
            line.push(format!("{lo:.1}..{hi:.1}"));
        }
        table.push(line);
    }
    print_markdown(&table, &[0, 2]);
    println!(
        "\n{connections} connections, {}, mean of {rounds} round{}.\n",
        mode(pipeline),
        if rounds == 1 { "" } else { "s" }
    );

    let load = mode(pipeline);
    for path in &paths {
        let ranked: Vec<(&str, f64)> = order
            .iter()
            .filter(|k| k.1 == *path)
            .map(|k| (k.0, rps(k)))
            .collect();
        let tag = format!("{path} ({load})");
        rank_high(&tag, &ranked, " req/s");
        let cpu = order
            .iter()
            .filter(|k| k.1 == *path)
            .map(|k| (k.0, mean(k, |r| r.cpu_us)))
            .collect();
        rank_low(&format!("{tag} CPU per request"), cpu, |v| {
            format!("{v:.1} µs")
        });
    }
    rank_own(rows);
}

/// the-benchmarker's board: each server's requests per second at each
/// level, the mean of its routes (their data.json averages the routes'
/// rates) and of the rounds, fastest first at the first level, as their
/// results page sorts at 64; beside it that level's p99, CPU per request and
/// cores kept busy (their saturation probe's figure). Then where Wisp ranks
/// at each level and on what is a server's own.
fn print_suite(rows: &[Row], levels: &[usize], cpus: usize) {
    let mut names = names(rows);
    let mean = |name: &str, level: usize, f: fn(&Row) -> f64| -> Option<f64> {
        let mine: Vec<f64> = rows
            .iter()
            .filter(|r| r.server == name && r.connections == level)
            .map(f)
            .collect();
        (!mine.is_empty()).then(|| mine.iter().sum::<f64>() / mine.len() as f64)
    };
    let rps = |name: &str, level: usize| mean(name, level, |r| r.rps).unwrap_or(0.0);
    let first = levels[0];
    names.sort_by(|a, b| rps(b, first).total_cmp(&rps(a, first)));

    let mut table = vec![vec!["#".to_string(), "Server".into()]];
    table[0].extend(levels.iter().map(|c| format!("req/s c={c}")));
    table[0].extend(["p99", "CPU µs/req", "Cores busy"].map(|what| format!("{what} c={first}")));
    for (i, &name) in names.iter().enumerate() {
        let mut line = vec![
            (i + 1).to_string(),
            if name == "Wisp" {
                "**Wisp**".to_string()
            } else {
                name.to_string()
            },
        ];
        line.extend(levels.iter().map(|&c| thousands(rps(name, c) as u64)));
        let at = |f: fn(&Row) -> f64| mean(name, first, f).unwrap_or(0.0);
        line.push(wisp_load::ms(at(|r| r.p99 as f64) as u64));
        line.push(format!("{:.1}", at(|r| r.cpu_us)));
        line.push(format!("{:.1} of {cpus}", at(|r| r.cpu_us * r.rps / 1e6)));
        table.push(line);
    }
    print_markdown(&table, &[1]);
    let routes: Vec<&str> = ROUTES.iter().map(|r| r.0).collect();
    println!(
        "\nreq/s is the mean of {}, closed loop.\n",
        routes.join(", ")
    );

    for &c in levels {
        let mut ranked: Vec<(&str, f64)> = names
            .iter()
            .filter_map(|&n| Some((n, mean(n, c, |r| r.rps)?)))
            .collect();
        ranked.sort_by(|a, b| b.1.total_cmp(&a.1));
        let headline = if c == 64 { " (their headline)" } else { "" };
        rank_high(
            &format!("the-benchmarker c={c}{headline}"),
            &ranked,
            " req/s",
        );
    }
    let cpu = names
        .iter()
        .filter_map(|&n| Some((n, mean(n, first, |r| r.cpu_us)?)))
        .collect();
    rank_low(
        &format!("the-benchmarker c={first} CPU per request"),
        cpu,
        |v| format!("{v:.1} µs"),
    );
    rank_own(rows);
}

/// `table` as Markdown, its first line the header: the `left` columns
/// (text) aligned left, the others (numbers) right.
fn print_markdown(table: &[Vec<String>], left: &[usize]) {
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
                if left.contains(&c) {
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
                    if left.contains(&c) {
                        "-".repeat(w + 2)
                    } else {
                        format!("{}:", "-".repeat(w + 1))
                    }
                })
                .collect();
            println!("|{}|", rules.join("|"));
        }
    }
}

/// One line on where Wisp ranks in `ranked`, a figure where more is better
/// (requests per second unless `unit` says otherwise), highest first,
/// against the best of the others (or the next, when Wisp leads).
fn rank_high(tag: &str, ranked: &[(&str, f64)], unit: &str) {
    let n = ranked.len();
    let Some(&(best, best_rps)) = ranked.first() else {
        return;
    };
    let best_text = thousands(best_rps as u64);
    match ranked.iter().position(|r| r.0 == "Wisp") {
        // One server's own path, like ASP.NET's /fortunes-blazor.
        None if n == 1 => {}
        None => {
            println!("rank {tag}: Wisp not measured; best of {n} is {best} ({best_text}{unit})")
        }
        Some(0) if n == 1 => println!("rank {tag}: Wisp alone, {best_text}{unit}"),
        Some(0) => {
            let (next, next_rps) = ranked[1];
            println!(
                "rank {tag}: Wisp 1st of {n}, {best_text}{unit}, {:.0}% ahead of {next} ({}{unit})",
                (best_rps / next_rps - 1.0) * 100.0,
                thousands(next_rps as u64)
            );
        }
        Some(i) => println!(
            "rank {tag}: Wisp {} of {n}, {}{unit}, {:.0}% of {best} ({best_text}{unit})",
            ordinal(i + 1),
            thousands(ranked[i].1 as u64),
            ranked[i].1 / best_rps * 100.0
        ),
    }
}

/// Where Wisp ranks on what is a server's own, whatever it served: peak
/// memory, time to first response and deploy size.
fn rank_own(rows: &[Row]) {
    let (mut memory, mut start, mut size) = (Vec::new(), Vec::new(), Vec::new());
    for name in names(rows) {
        let mine: Vec<&Row> = rows.iter().filter(|r| r.server == name).collect();
        memory.push((name, mine.iter().map(|r| r.peak_mb).fold(0.0, f64::max)));
        let starts: f64 = mine.iter().map(|r| r.start_ms as f64).sum();
        start.push((name, starts / mine.len() as f64));
        if let Some(bytes) = mine[0].size {
            size.push((name, bytes as f64));
        }
    }
    rank_low("peak memory", memory, |v| format!("{v:.1} MB"));
    rank_low("first response", start, |v| format!("{v:.0} ms"));
    rank_low("deploy size (runtime not counted)", size, |v| {
        size_text(v as u64)
    });
}

/// The servers in `rows`, in the order they ran.
fn names(rows: &[Row]) -> Vec<&'static str> {
    let mut names = Vec::new();
    for r in rows {
        if !names.contains(&r.server) {
            names.push(r.server);
        }
    }
    names
}

/// One line on where Wisp ranks in `all`, a figure where less is better,
/// against the best of the others (or the next, when Wisp leads).
fn rank_low(what: &str, mut all: Vec<(&str, f64)>, show: impl Fn(f64) -> String) {
    all.sort_by(|a, b| a.1.total_cmp(&b.1));
    let n = all.len();
    let Some(i) = all.iter().position(|r| r.0 == "Wisp").filter(|_| n > 1) else {
        return;
    };
    let (other, kind) = if i == 0 {
        (all[1], "next")
    } else {
        (all[0], "best")
    };
    println!(
        "rank {what}: Wisp {} of {n}, {}, {kind} {} ({})",
        ordinal(i + 1),
        show(all[i].1),
        other.0,
        show(other.1)
    );
}

/// `1234567` → `1.2 MB`; under a megabyte, in KB.
fn size_text(bytes: u64) -> String {
    if bytes < 1 << 20 {
        format!("{} KB", bytes.div_ceil(1024))
    } else {
        format!("{:.1} MB", bytes as f64 / (1 << 20) as f64)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_text_is_compared_unescaped() {
        let name = "Ada <&\"";
        assert_eq!(unescape("Ada &lt;&amp;&quot;").as_deref(), Some(name));
        assert_eq!(unescape("Ada &#60;&#38;&#34;").as_deref(), Some(name));
        assert_eq!(unescape("Ada &#x3C;&#x26;&#x22;").as_deref(), Some(name));
        assert_eq!(unescape("Ada &lt;&amp;\"").as_deref(), Some(name));
        assert_eq!(unescape("Ada <&\""), None, "raw <");
        assert_eq!(unescape("Ada &lt;&\""), None, "bare &");
        assert_eq!(without_comments("a<!---->b<!--[-->c<!--]-->"), "abc");
    }

    #[test]
    fn zrk_is_read_and_imitated() {
        let report = "{\n  \"requests\": 3000,\n  \"bytes\": 240000,\n  \"achieved_rate\": 200.50,\n  \"config\": { \"timeout_ms\": 8000 },\n  \"latency_us\": {\n    \"p50\": 250, \"p99\": 900, \"p99_9\": 1500, \"p99_99\": 2000\n  },\n  \"errors\": { \"connect\": 0, \"read\": 1, \"timeout\": 2, \"non_2xx_3xx\": 0 },\n}";
        assert_eq!(number(report, "achieved_rate"), Some(200.5));
        assert_eq!(number(report, "p99"), Some(900.0));
        assert_eq!(number(report, "p99_9"), Some(1500.0));
        assert_eq!(number(report, "timeout"), Some(2.0));
        assert_eq!(number(report, "non_2xx_3xx"), Some(0.0));
        assert_eq!(number(report, "missing"), None);
        let addr: SocketAddr = ([127, 0, 0, 1], 3000).into();
        assert_eq!(
            zrk_request(addr, "POST /user"),
            "POST /user HTTP/1.1\r\nHost: 127.0.0.1:3000\r\nUser-Agent: zrk\r\nConnection: keep-alive\r\nContent-Length: 0\r\n\r\n"
        );
        assert!(zrk_request(addr, "GET /user/0").starts_with("GET /user/0 HTTP/1.1\r\n"));
    }

    #[test]
    fn sizes() {
        assert_eq!(size_text(1500), "2 KB");
        assert_eq!(size_text(6 << 20), "6.0 MB");
    }
}
