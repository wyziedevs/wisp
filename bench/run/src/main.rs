//! Wisp against other web stacks on this machine, on Linux or Windows.
//!
//! Builds every server in release mode, then runs each one alone on half of
//! the CPU cores with the load generator on the other half, so the two never
//! compete for a core. Prints throughput, latency, CPU time per request,
//! response size, peak memory and time to first response, then a Markdown
//! table of it all.
//!
//!   cargo run -r -p bench-run -- [-c 64] [-d 10] [-w 5] [--rounds 1]
//!       [--only wisp,actix] [--paths fortunes] [--no-build] [--csv FILE]
//!       [--extra NAME=COMMAND]...
//!
//! `--only` and `--paths` pick servers and paths by case-insensitive
//! substring. `--extra NAME=COMMAND` adds a server of your own, measured on
//! `/plaintext` only; it gets `PORT` and `THREADS` like the others. `--rounds` runs every server that many times, taking turns,
//! and reports the mean. `--csv` appends every run to a file. A server whose
//! toolchain is not installed is skipped with a note.

mod sys;

use std::collections::HashMap;
use std::io::Write;
use std::net::SocketAddr;
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
    /// `--extra`: `args[0]` is the program.
    Other,
}

struct Server {
    name: &'static str,
    bin: Bin,
    args: &'static [&'static str],
    /// The variable that sets the server's thread or process count.
    threads: &'static str,
    env: &'static [(&'static str, &'static str)],
    /// Paths it serves besides `/plaintext` and `/fortunes`.
    extra: &'static [&'static str],
}

const SERVERS: &[Server] = &[
    Server {
        name: "Wisp",
        bin: Bin::Wisp,
        args: &[],
        threads: "WISP_THREADS",
        env: &[("HOST", "127.0.0.1")],
        extra: &[],
    },
    Server {
        name: "ASP.NET Core",
        bin: Bin::AspNet,
        args: &[],
        threads: "",
        env: &[],
        extra: &["/fortunes-blazor"],
    },
    Server {
        name: "Actix Web",
        bin: Bin::Rust,
        args: &["actix"],
        threads: "THREADS",
        env: &[],
        extra: &[],
    },
    Server {
        name: "Axum",
        bin: Bin::Rust,
        args: &["axum"],
        threads: "THREADS",
        env: &[],
        extra: &[],
    },
    Server {
        name: "Go net/http",
        bin: Bin::Go,
        args: &["nethttp"],
        threads: "GOMAXPROCS",
        env: &[],
        extra: &[],
    },
    Server {
        name: "Fiber",
        bin: Bin::Go,
        args: &["fiber"],
        threads: "GOMAXPROCS",
        env: &[],
        extra: &[],
    },
    Server {
        name: "Fastify",
        bin: Bin::Node,
        args: &["cluster.mjs", "fastify/server.mjs"],
        threads: "WORKERS",
        env: &[],
        extra: &[],
    },
    Server {
        name: "SvelteKit",
        bin: Bin::Node,
        args: &["cluster.mjs", "sveltekit/build/index.js"],
        threads: "WORKERS",
        env: &[("HOST", "127.0.0.1")],
        extra: &[],
    },
    Server {
        name: "Next.js",
        bin: Bin::Node,
        args: &["cluster.mjs", "nextjs/.next/standalone/nextjs/server.js"],
        threads: "WORKERS",
        env: &[("HOSTNAME", "127.0.0.1")],
        extra: &[],
    },
];

struct Options {
    connections: usize,
    duration: Duration,
    warmup: Duration,
    rounds: usize,
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
        .filter(|s| picked(&opt.only, s.name))
        .collect();
    servers.retain(|s| {
        let tool = toolchain(s.bin);
        let found = installed(tool);
        if !found {
            println!("skipping {}: `{tool}` is not installed", s.name);
        }
        found
    });
    servers.extend(&opt.extra);
    if servers.is_empty() {
        die("no servers to run");
    }
    if opt.build {
        build(&repo, &bench, &servers);
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
                &["/plaintext", "/fortunes"]
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
            let mut child = start(s, &repo, &bench, port, &threads, &server_cpus);
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
                });
            }
            "-h" | "--help" => {
                println!(
                    "usage: bench-run [-c 64] [-d 10] [-w 5] [--rounds 1] [--only wisp,actix] [--paths fortunes] [--no-build] [--csv FILE] [--extra NAME=COMMAND]..."
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

fn toolchain(bin: Bin) -> &'static str {
    match bin {
        Bin::Wisp | Bin::Rust => "cargo",
        Bin::Go => "go",
        Bin::AspNet => "dotnet",
        Bin::Node => "node",
        Bin::Other => "",
    }
}

fn installed(tool: &str) -> bool {
    if tool.is_empty() {
        return true;
    }
    let arg = if tool == "go" { "version" } else { "--version" };
    Command::new(tool)
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

fn build(repo: &Path, bench: &Path, servers: &[&Server]) {
    let has = |bin: Bin| servers.iter().any(|s| s.bin == bin);
    let has_node = |app: &str| {
        servers
            .iter()
            .any(|s| s.args.get(1).is_some_and(|a| a.starts_with(app)))
    };
    let npm = if cfg!(windows) { "npm.cmd" } else { "npm" };
    if has(Bin::Wisp) {
        run(Command::new("cargo")
            .args(["build", "--release", "-q", "-p", "wisp-bench"])
            .current_dir(repo));
    }
    if has(Bin::Rust) {
        run(Command::new("cargo")
            .args(["build", "--release", "-q"])
            .current_dir(bench.join("rust")));
    }
    if has(Bin::AspNet) {
        run(Command::new("dotnet")
            .args([
                "publish", "-c", "Release", "-o", "out", "--nologo", "-v", "q",
            ])
            .current_dir(bench.join("aspnet")));
    }
    if has(Bin::Go) {
        run(Command::new("go")
            .args([
                "build",
                "-mod=mod",
                "-o",
                &format!("out/{}", exe("bench-go")),
                ".",
            ])
            .current_dir(bench.join("go")));
    }
    for app in ["fastify", "sveltekit", "nextjs"] {
        if !has_node(app) {
            continue;
        }
        let dir = bench.join("node").join(app);
        if !dir.join("node_modules").exists() {
            run(Command::new(npm)
                .args(["install", "--no-audit", "--no-fund", "--loglevel=error"])
                .current_dir(&dir));
        }
        if app != "fastify" {
            run(Command::new(npm)
                .args(["run", "build", "--silent"])
                .env("NEXT_TELEMETRY_DISABLED", "1")
                .current_dir(&dir));
        }
    }
}

fn run(cmd: &mut Command) {
    println!(
        "› {} {}",
        cmd.get_program().to_string_lossy(),
        cmd.get_args()
            .map(|a| a.to_string_lossy())
            .collect::<Vec<_>>()
            .join(" ")
    );
    match cmd.status() {
        Ok(s) if s.success() => {}
        Ok(s) => die(&format!("build failed ({s})")),
        Err(e) => die(&format!("could not run the build: {e}")),
    }
}

fn start(s: &Server, repo: &Path, bench: &Path, port: u16, threads: &str, cpus: &[usize]) -> Child {
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
    sys::spawn_pinned(&mut cmd, cpus).unwrap_or_else(|e| {
        die(&format!(
            "{}: cannot start {}: {e}",
            s.name,
            program.display()
        ))
    })
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

/// The mean of every round per server and path, as a Markdown table.
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
    let mut table = vec![vec![
        "Server".to_string(),
        "Path".into(),
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
    for key in order {
        let g = &groups[&key];
        let mean = |f: fn(&Row) -> f64| g.iter().map(|r| f(r)).sum::<f64>() / g.len() as f64;
        let mut line = vec![
            key.0.to_string(),
            key.1.to_string(),
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
                if c < 2 {
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
                    if c < 2 {
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
        "\n{connections} connections, mean of {rounds} round{}.",
        if rounds == 1 { "" } else { "s" }
    );
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
