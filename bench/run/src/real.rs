//! `--suite real`: what a server does in practice, rather than its peak
//! closed-loop rate. On each server, in this order (`--tests` picks some):
//!
//! Traffic, on paths every server answers:
//! - users: how many people it serves at once within a p99, each a
//!   keep-alive connection sending a request every `--think` seconds; then
//!   twice that many, to see whether it degrades or collapses past it.
//! - churn: a new connection per request, as clients without keep-alive,
//!   health checks and some proxies send.
//! - slow: 2,000 slowloris clients beside 64 normal connections.
//!
//! An app's own work, on the practice routes (bench/README.md); "n/a" where
//! a server has none:
//! - wait: 1,000 connections on a handler that waits 20 ms, whether a
//!   waiting handler holds up the others (50k req/s is ideal).
//! - echo: JSON in, validated, JSON out. list: 1,000 rows of JSON out.
//! - upload: 1 MiB bodies, and a 9 MiB one refused. static: a 100 KB file.
//! - ws: 10,000 idle WebSockets, 64 of them echoing.
//!
//! Robustness:
//! - abuse: five malformed requests, after which it must still answer.
//! - soak: users at half its most for `--soak` seconds: does memory grow?
//! - shutdown: SIGTERM under load: are requests in flight answered?

use crate::{
    Options, Server, announce, check_fortunes, check_page, cpu_used, die, print_markdown,
    rank_high, rank_low, sys, thousands, wait_ready,
};
use std::io::Write;
use std::net::SocketAddr;
use std::path::Path;
use std::process::Child;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use wisp_load::{Answer, Report, Step, Users, ms};

/// Every test, in the order they run: the WebSockets' memory is read
/// before uploads swell a garbage-collected server's heap.
pub const TESTS: [&str; 12] = [
    "users", "churn", "slow", "ws", "wait", "echo", "list", "static", "upload", "abuse", "soak",
    "shutdown",
];

/// Users in the first step.
const START: usize = 500;
/// From a step's new users to its measuring: they connect over a second.
const SETTLE: Duration = Duration::from_secs(2);
/// Past this p99 of its own lag, the load generator, not the server, sets
/// the pace: the ramp stops there.
const LATE_US: u64 = 10_000;
/// Connections in churn, beside the slow clients, and on most app routes.
const CONNECTIONS: usize = 64;
const SLOW_CLIENTS: usize = 2000;
const WAITING: usize = 1000;
const UPLOADS: usize = 32;
const SOCKETS: usize = 10_000;
/// Before each closed loop on an app route.
const WARMUP: Duration = Duration::from_secs(1);
/// `/static/app.js`'s length.
const APP_JS: usize = 100_253;
/// A valid `/echo` body, about 200 bytes, and one with every field wrong.
const ECHO: &str = r#"{"name":"Ada Lovelace","email":"ada.lovelace@example.com","age":36,"tags":["mathematics","analytical-engine","poetry","notes","bernoulli","computing","babbage","london"]}"#;
const ECHO_BAD: &str = r#"{"name":"","email":"nope","age":200,"tags":[]}"#;

/// One server's results: `None` where a test did not run (or the server
/// was gone by then), `Some(None)` where the server lacks the route.
#[derive(Default)]
pub struct Row {
    server: &'static str,
    users: Option<Crowd>,
    /// Connections a second, and CPU µs each.
    churn: Option<(f64, f64)>,
    /// The normal connections' rate beside the slow clients as % of theirs
    /// alone, their p99, and how many slow clients the server cut off.
    slow: Option<(f64, u64, usize)>,
    /// req/s, p50, p99.
    wait: Option<Option<(f64, u64, u64)>>,
    /// req/s, CPU µs per request.
    echo: Option<Option<(f64, f64)>>,
    /// MB/s in, and the most resident MB while uploading.
    upload: Option<Option<(f64, f64)>>,
    list: Option<Option<(f64, f64)>>,
    static_file: Option<Option<f64>>,
    /// KB per idle socket, echoes a second, p99.
    ws: Option<Option<(f64, f64, u64)>>,
    /// Malformed requests handled, and what went wrong.
    abuse: Option<(usize, Vec<String>)>,
    /// Resident MB at the first and the last sample.
    soak: Option<Option<(f64, f64)>>,
    /// Requests in flight that failed, and ms until it exited.
    shutdown: Option<Option<(u64, u64)>>,
}

/// The users test's results.
#[derive(Clone, Copy)]
struct Crowd {
    /// The most users that passed, 0 if none did.
    users: usize,
    /// The load generator fell behind first: the server serves at least
    /// `users`.
    limited: bool,
    /// At `users`: p99 in µs, CPU % of the server's CPUs, resident MB, and
    /// KB per user over the server idle.
    p99: u64,
    cpu: f64,
    mb: f64,
    kb_per_user: f64,
    /// At twice `users`: % of the requests due that were served, and p99.
    over: Option<(f64, u64)>,
}

/// One step of users, measured.
struct Measured {
    users: usize,
    pass: bool,
    p99: u64,
    late: u64,
    served: f64,
    cpu: f64,
    rss: u64,
}

/// What every test needs of the server.
struct Ctx<'a> {
    addr: SocketAddr,
    opt: &'a Options,
    tree: Vec<u32>,
    /// The server's CPUs, and the load's threads.
    cpus: usize,
    threads: usize,
}

impl Ctx<'_> {
    /// Bytes the server's processes have resident now.
    fn rss(&self) -> u64 {
        self.tree.iter().map(|&pid| sys::memory(pid)).sum()
    }

    /// `request` closed loop on `connections` after a warmup, and the
    /// server's CPU µs per request.
    fn load(&self, request: &[u8], connections: usize) -> (Report, f64) {
        let before = sys::cpu_times(&self.tree);
        let r = wisp_load::closed_loop(
            self.addr,
            request,
            connections,
            self.threads,
            WARMUP,
            self.opt.duration,
        );
        let (cpu, _) = cpu_used(&before, &sys::cpu_times(&self.tree));
        // CPU over the warmup too, so per request over all sent.
        let all = r.ok as f64 * (WARMUP + self.opt.duration).as_secs_f64() / r.seconds;
        (r, cpu * 1e6 / all.max(1.0))
    }

    /// Runs `f`, sampling resident memory every 100 ms: what `f` returned
    /// and the most seen.
    fn sampled<T: Send>(&self, f: impl FnOnce() -> T + Send) -> (T, u64) {
        let done = AtomicBool::new(false);
        std::thread::scope(|s| {
            let h = s.spawn(|| {
                let r = f();
                done.store(true, Ordering::Relaxed);
                r
            });
            let mut most = 0;
            while !done.load(Ordering::Relaxed) {
                most = most.max(self.rss());
                std::thread::sleep(Duration::from_millis(100));
            }
            (h.join().expect("load thread"), most)
        })
    }

    fn get(&self, path: &str) -> String {
        wisp_load::get_request(self.addr, path)
    }

    fn post(&self, path: &str, kind: &str, body: &str) -> String {
        format!(
            "POST {path} HTTP/1.1\r\nhost: {}\r\ncontent-type: {kind}\r\ncontent-length: {}\r\n\r\n{body}",
            self.addr,
            body.len()
        )
    }

    /// Sends `request` once and checks the answer with `ok`; `Err` says
    /// why not, "404" when there is no such route.
    fn check(&self, request: &str, ok: impl Fn(u16, &[u8]) -> bool) -> Result<(), String> {
        match wisp_load::send(self.addr, request, Duration::from_secs(5)) {
            Some((status, body)) if ok(status, &body) => Ok(()),
            Some((404, _)) => Err("404".into()),
            Some((status, body)) => Err(format!(
                "answered {status} {:?}",
                String::from_utf8_lossy(&body[..body.len().min(80)])
            )),
            None => Err("no answer".into()),
        }
    }
}

/// The suite's header line.
pub fn announce_suite(opt: &Options, server_cpus: &[usize], load_cpus: &[usize]) {
    let mix: Vec<String> = wisp_load::MIX
        .iter()
        .map(|(path, tenths)| format!("{path} {}%", tenths * 10))
        .collect();
    println!(
        "real suite: servers on CPUs {}, load on {}; tests {}; users send every {:.1} s ±50% ({}), pass at p99 ≤ {} ms with ≤ 0.1% failed and ≥ 99% served, from {START} users to {} at most, {}s settle + {}s a step; {}s a load elsewhere; soak {}s",
        sys::list(server_cpus),
        sys::list(load_cpus),
        opt.tests.join(","),
        opt.think.as_secs_f64(),
        mix.join(", "),
        opt.slo_ms,
        thousands(opt.max_users as u64),
        SETTLE.as_secs(),
        opt.duration.as_secs(),
        opt.duration.as_secs(),
        opt.soak.as_secs(),
    );
}

pub fn runs(opt: &Options, test: &str) -> bool {
    opt.tests.iter().any(|t| t == test)
}

/// Checks the server sends what the others do, then runs the tests picked,
/// until one finds it gone.
pub fn measure(
    s: &Server,
    child: &mut Child,
    addr: SocketAddr,
    opt: &Options,
    cpus: usize,
    threads: usize,
) -> Result<Row, String> {
    let start_ms = wait_ready(child, addr, &wisp_load::get_request(addr, "/json"))?;
    check_fortunes(addr, "/fortunes")?;
    check_page(addr, "/page")?;
    let cx = Ctx {
        addr,
        opt,
        tree: sys::tree(child.id()),
        cpus,
        threads,
    };
    announce(s, start_ms, cx.tree.len());
    let mut row = Row {
        server: s.name,
        ..Row::default()
    };
    for &test in TESTS.iter().filter(|t| runs(opt, t)) {
        match test {
            "users" => match users(&cx, child) {
                Ok(c) => row.users = Some(c),
                Err(e) => println!("  users: {e}"),
            },
            "churn" => row.churn = Some(churn(&cx)),
            "slow" => row.slow = Some(slow(&cx)),
            "wait" => row.wait = Some(na(test, wait(&cx))),
            "echo" => row.echo = Some(na(test, echo(&cx))),
            "upload" => row.upload = Some(na(test, upload(&cx))),
            "list" => row.list = Some(na(test, list(&cx))),
            "static" => row.static_file = Some(na(test, static_file(&cx))),
            "ws" => row.ws = Some(na(test, ws(&cx))),
            "abuse" => row.abuse = Some(abuse(&cx, child)),
            "soak" => row.soak = Some(na(test, soak(&cx, row.users.map(|c| c.users)))),
            "shutdown" => row.shutdown = Some(na(test, shutdown(&cx, child))),
            _ => unreachable!("TESTS"),
        }
        if test != "shutdown"
            && let Ok(Some(status)) = child.try_wait()
        {
            println!("  exited during {test} ({status})");
            break;
        }
    }
    Ok(row)
}

/// What a test on a route some servers lack found: n/a, with why, if it
/// could not run.
fn na<T>(test: &str, r: Result<T, String>) -> Option<T> {
    r.map_err(|e| println!("  {test}: n/a ({e})")).ok()
}

/// Users, from 500 up while they pass, then twice the most that did.
fn users(cx: &Ctx, child: &mut Child) -> Result<Crowd, String> {
    let idle = cx.rss();
    let users = Users::start(cx.addr, cx.threads, cx.opt.think);
    let mut ramp = Ramp::new(cx.opt.max_users);
    let mut n = ramp.first();
    let mut best: Option<Measured> = None;
    let mut limited = false;
    loop {
        let m = step(&users, n, child, cx)?;
        let (pass, late) = (m.pass, m.late > LATE_US);
        if pass && best.as_ref().is_none_or(|b| n > b.users) {
            best = Some(m);
        }
        if late {
            limited = true;
            break;
        }
        match ramp.next(n, pass) {
            Some(next) => n = next,
            None => break,
        }
    }
    // Past the limit, unless the load was the limit.
    let over = match &best {
        Some(b) if !limited => {
            let m = step(&users, 2 * b.users, child, cx)?;
            Some((m.served, m.p99))
        }
        _ => None,
    };
    let (users, p99, cpu, rss) = best.map_or((0, 0, 0.0, idle), |b| (b.users, b.p99, b.cpu, b.rss));
    Ok(Crowd {
        users,
        limited,
        p99,
        cpu,
        mb: rss as f64 / (1 << 20) as f64,
        kb_per_user: rss.saturating_sub(idle) as f64 / 1024.0 / users.max(1) as f64,
        over,
    })
}

/// `n` users, settled, then measured, and a line on it.
fn step(users: &Users, n: usize, child: &mut Child, cx: &Ctx) -> Result<Measured, String> {
    users.resize(n, SETTLE);
    let before = sys::cpu_times(&cx.tree);
    let wall = Instant::now();
    let r: Step = users.measure(cx.opt.duration);
    let (cpu, _) = cpu_used(&before, &sys::cpu_times(&cx.tree));
    let cpu = cpu / wall.elapsed().as_secs_f64() / cx.cpus as f64 * 100.0;
    if let Ok(Some(status)) = child.try_wait() {
        return Err(format!("exited at {n} users ({status})"));
    }
    let m = Measured {
        users: n,
        pass: r.passes(cx.opt.slo_ms * 1000),
        p99: r.latency.percentile(0.99),
        late: r.late.percentile(0.99),
        served: r.ok as f64 / r.scheduled.max(1) as f64 * 100.0,
        cpu,
        rss: cx.rss(),
    };
    println!(
        "  {:>7} users  p99 {:>9}  served {:5.1}%  CPU {:5.1}%  {:5.0} MB  load late p99 {}  {}{}",
        thousands(n as u64),
        ms(m.p99),
        m.served,
        m.cpu,
        m.rss as f64 / (1 << 20) as f64,
        ms(m.late),
        if m.late > LATE_US {
            "load-limited"
        } else if m.pass {
            "pass"
        } else {
            "fail"
        },
        failures(r.non_2xx + r.errors)
    );
    Ok(m)
}

fn churn(cx: &Ctx) -> (f64, f64) {
    let close = format!(
        "GET /json HTTP/1.1\r\nhost: {}\r\naccept: */*\r\nconnection: close\r\n\r\n",
        cx.addr
    );
    let before = sys::cpu_times(&cx.tree);
    let r = wisp_load::churn(cx.addr, &close, CONNECTIONS, cx.threads, cx.opt.duration);
    let (cpu, _) = cpu_used(&before, &sys::cpu_times(&cx.tree));
    let us = cpu * 1e6 / r.ok.max(1) as f64;
    println!(
        "  churn: {:.0} connections/s  p99 {}  {us:.1} µs CPU each{}",
        r.rps(),
        ms(r.latency.percentile(0.99)),
        failures(r.non_2xx + r.errors)
    );
    (r.rps(), us)
}

fn slow(cx: &Ctx) -> (f64, u64, usize) {
    let page = cx.get("/page");
    let d = cx.opt.duration;
    let alone = wisp_load::run(cx.addr, &page, CONNECTIONS, 1, WARMUP, d);
    let (cut, beside) = std::thread::scope(|sc| {
        let held = SETTLE + WARMUP + d + Duration::from_millis(500);
        let (addr, threads) = (cx.addr, cx.threads);
        let slow = sc.spawn(move || wisp_load::slow(addr, "/page", SLOW_CLIENTS, threads, held));
        std::thread::sleep(SETTLE);
        let r = wisp_load::run(cx.addr, &page, CONNECTIONS, 1, WARMUP, d);
        (slow.join().expect("slow clients"), r)
    });
    let kept = beside.rps() / alone.rps().max(1.0) * 100.0;
    let p99 = beside.latency.percentile(0.99);
    println!(
        "  slow: /page {:.0} req/s beside {} slow clients, {kept:.0}% of {:.0} alone, p99 {} (alone {}), {cut} slow cut{}",
        beside.rps(),
        thousands(SLOW_CLIENTS as u64),
        alone.rps(),
        ms(p99),
        ms(alone.latency.percentile(0.99)),
        failures(beside.non_2xx + beside.errors)
    );
    (kept, p99, cut)
}

fn wait(cx: &Ctx) -> Result<(f64, u64, u64), String> {
    let request = cx.get("/wait");
    cx.check(&request, |status, body| {
        status == 200 && contains(body, "ok")
    })?;
    let (r, _) = cx.load(request.as_bytes(), WAITING);
    let (p50, p99) = (r.latency.percentile(0.5), r.latency.percentile(0.99));
    println!(
        "  wait: {:.0} req/s at {WAITING} connections (ideal {}), p50 {}, p99 {}{}",
        r.rps(),
        thousands((WAITING * 50) as u64),
        ms(p50),
        ms(p99),
        failures(r.non_2xx + r.errors)
    );
    Ok((r.rps(), p50, p99))
}

fn echo(cx: &Ctx) -> Result<(f64, f64), String> {
    let json = "application/json";
    cx.check(&cx.post("/echo", json, ECHO_BAD), |status, _| {
        (400..500).contains(&status) && status != 404
    })?;
    let request = cx.post("/echo", json, ECHO);
    cx.check(&request, |status, body| {
        status == 200 && contains(body, "ada.lovelace@example.com")
    })?;
    let (r, us) = cx.load(request.as_bytes(), CONNECTIONS);
    println!(
        "  echo: {:.0} req/s, {us:.1} µs CPU each{}",
        r.rps(),
        failures(r.non_2xx + r.errors)
    );
    Ok((r.rps(), us))
}

fn upload(cx: &Ctx) -> Result<(f64, f64), String> {
    let kind = "application/octet-stream";
    let big = cx.post("/upload", kind, &"x".repeat(9 << 20));
    match wisp_load::raw(cx.addr, big.as_bytes(), Duration::from_secs(5)) {
        Answer::Status(413) | Answer::Closed => {}
        Answer::Status(404) => return Err("404".into()),
        Answer::Status(status) => return Err(format!("9 MiB answered {status}, not 413")),
        Answer::Silent => return Err("9 MiB had no answer in 5 s".into()),
    }
    cx.check(&cx.get("/json"), |status, _| status == 200)
        .map_err(|e| format!("after 9 MiB, /json: {e}"))?;
    let request = cx.post("/upload", kind, &"x".repeat(1 << 20));
    cx.check(&request, |status, body| {
        status == 200 && String::from_utf8_lossy(body).trim() == (1 << 20).to_string()
    })?;
    let ((r, _), peak) = cx.sampled(|| cx.load(request.as_bytes(), UPLOADS));
    let mbs = r.rps() * (1 << 20) as f64 / 1e6;
    let peak = peak as f64 / (1 << 20) as f64;
    println!(
        "  upload: {mbs:.0} MB/s in 1 MiB bodies at {UPLOADS} connections, {peak:.0} MB at most; 9 MiB refused{}",
        failures(r.non_2xx + r.errors)
    );
    Ok((mbs, peak))
}

fn list(cx: &Ctx) -> Result<(f64, f64), String> {
    let request = cx.get("/list");
    cx.check(&request, |status, body| {
        status == 200 && contains(body, "user999@example.com")
    })?;
    let (r, us) = cx.load(request.as_bytes(), CONNECTIONS);
    println!(
        "  list: {:.0} req/s, {us:.1} µs CPU each{}",
        r.rps(),
        failures(r.non_2xx + r.errors)
    );
    Ok((r.rps(), us))
}

fn static_file(cx: &Ctx) -> Result<f64, String> {
    let request = cx.get("/static/app.js");
    cx.check(&request, |status, body| {
        status == 200 && body.len() == APP_JS
    })?;
    let (r, us) = cx.load(request.as_bytes(), CONNECTIONS);
    println!(
        "  static: {:.0} req/s ({:.0} MB/s), {us:.1} µs CPU each{}",
        r.rps(),
        r.bytes as f64 / r.seconds / 1e6,
        failures(r.non_2xx + r.errors)
    );
    Ok(r.rps())
}

fn ws(cx: &Ctx) -> Result<(f64, f64, u64), String> {
    wisp_load::ws_check(cx.addr, "/ws")?;
    let before = cx.rss();
    let mut open = 0;
    let r = wisp_load::ws(
        cx.addr,
        "/ws",
        SOCKETS,
        CONNECTIONS,
        cx.threads,
        cx.opt.duration,
        || open = cx.rss(),
    );
    // A garbage-collected server may give back more than they took.
    let kb = open.saturating_sub(before) as f64 / 1024.0 / r.opened.max(1) as f64;
    let p99 = r.echo.latency.percentile(0.99);
    println!(
        "  ws: {} of {} sockets open, {:.0} → {:.0} MB, {kb:.1} KB each; {CONNECTIONS} echoing {:.0} messages/s, p99 {}{}",
        thousands(r.opened as u64),
        thousands(SOCKETS as u64),
        before as f64 / (1 << 20) as f64,
        open as f64 / (1 << 20) as f64,
        r.echo.rps(),
        ms(p99),
        failures(r.echo.non_2xx + r.echo.errors)
    );
    Ok((kb, r.echo.rps(), p99))
}

/// A malformed request: its name, its bytes, and whether an answer
/// handles it.
type Abuse = (&'static str, Vec<u8>, fn(&Answer) -> bool);

/// The malformed requests, and whether a server's answer to each handles
/// it.
fn abuse_cases(addr: SocketAddr) -> Vec<Abuse> {
    let refused = |a: &Answer| matches!(a, Answer::Closed | Answer::Status(400..=499));
    let header = |n: usize| {
        format!(
            "GET /json HTTP/1.1\r\nhost: {addr}\r\nx-big: {}\r\n\r\n",
            "a".repeat(n)
        )
        .into_bytes()
    };
    vec![
        (
            "garbage request line",
            b"\x16\x03\x01\x02\x00 \xff\xfe nonsense \x00\r\n\r\n".to_vec(),
            refused,
        ),
        // Within some servers' limits (Go's is 1 MB): answering it is fine.
        ("64 KB header", header(64 << 10), |a| {
            matches!(a, Answer::Closed | Answer::Status(200..=499))
        }),
        ("1 MB header", header(1 << 20), refused),
        (
            "bad chunked body",
            format!("POST /echo HTTP/1.1\r\nhost: {addr}\r\ncontent-type: application/json\r\ntransfer-encoding: chunked\r\n\r\nzz\r\n{{}}\r\n0\r\n\r\n").into_bytes(),
            refused,
        ),
        ("no host", b"GET /json HTTP/1.1\r\n\r\n".to_vec(), refused),
    ]
}

/// Each malformed request on a connection of its own, 2 s for an answer;
/// then `/json` must answer within a second.
fn abuse(cx: &Ctx, child: &mut Child) -> (usize, Vec<String>) {
    let cases = abuse_cases(cx.addr);
    let mut failed = Vec::new();
    let mut handled = 0;
    for (name, bytes, ok) in &cases {
        let answer = wisp_load::raw(cx.addr, bytes, Duration::from_secs(2));
        let alive = matches!(child.try_wait(), Ok(None));
        if ok(&answer) && alive {
            handled += 1;
        } else {
            failed.push(match answer {
                _ if !alive => format!("{name}: exited"),
                Answer::Status(status) => format!("{name}: {status}"),
                Answer::Closed => format!("{name}: closed"),
                Answer::Silent => format!("{name}: no answer"),
            });
        }
    }
    let json = wisp_load::send(cx.addr, &cx.get("/json"), Duration::from_secs(1));
    if !matches!(json, Some((200, _))) {
        failed.push("then /json: no 200 in 1 s".into());
    }
    println!(
        "  abuse: {handled}/{} handled{}",
        cases.len(),
        if failed.is_empty() {
            String::new()
        } else {
            format!("; {}", failed.join(", "))
        }
    );
    (handled, failed)
}

/// Users at half the most that passed (or 1,000, if the users test did not
/// run) for `--soak`, resident memory sampled every 5 s.
fn soak(cx: &Ctx, most: Option<usize>) -> Result<(f64, f64), String> {
    if cx.opt.soak.is_zero() {
        return Err("--soak 0".into());
    }
    let n = most.map_or(1000, |m| m / 2);
    if n == 0 {
        return Err("no users passed".into());
    }
    let users = Users::start(cx.addr, cx.threads, cx.opt.think);
    users.resize(n, SETTLE);
    let mut samples = Vec::new();
    let r = std::thread::scope(|s| {
        let h = s.spawn(|| users.measure(cx.opt.soak));
        let t = Instant::now();
        while t.elapsed() <= cx.opt.soak {
            samples.push(cx.rss() as f64 / (1 << 20) as f64);
            std::thread::sleep(Duration::from_secs(5).min(cx.opt.soak));
        }
        h.join().expect("soak users")
    });
    let (first, last) = (samples[0], samples[samples.len() - 1]);
    println!(
        "  soak: {} users for {} s: {first:.0} → {last:.0} MB ({:+.0}%{}), p99 {}, served {:.1}%{}",
        thousands(n as u64),
        cx.opt.soak.as_secs(),
        drift(first, last),
        if grows(first, last) { ", grows" } else { "" },
        ms(r.latency.percentile(0.99)),
        r.ok as f64 / r.scheduled.max(1) as f64 * 100.0,
        failures(r.non_2xx + r.errors)
    );
    Ok((first, last))
}

/// Growth from `first` to `last`, in %.
fn drift(first: f64, last: f64) -> f64 {
    (last / first.max(1e-9) - 1.0) * 100.0
}

/// Memory that grew by more than a fifth over the soak.
fn grows(first: f64, last: f64) -> bool {
    drift(first, last) > 20.0
}

/// SIGTERM after a second of 64 connections on `/wait`: how many requests
/// sent before it failed, and how long the server took to exit (10 s at
/// most, then it is killed). Last, as it stops the server.
fn shutdown(cx: &Ctx, child: &mut Child) -> Result<(u64, u64), String> {
    if cfg!(windows) {
        return Err("no SIGTERM on Windows".into());
    }
    let request = cx.get("/wait");
    cx.check(&request, |status, _| status == 200)?;
    let (r, exit_ms) = std::thread::scope(|s| {
        let (addr, threads) = (cx.addr, cx.threads);
        let request = request.as_bytes();
        let load = s.spawn(move || {
            wisp_load::closed_loop(addr, request, CONNECTIONS, threads, WARMUP, WARMUP)
        });
        std::thread::sleep(2 * WARMUP);
        sys::terminate(child);
        let t = Instant::now();
        while matches!(child.try_wait(), Ok(None)) && t.elapsed() < Duration::from_secs(10) {
            std::thread::sleep(Duration::from_millis(5));
        }
        let exit_ms = t.elapsed().as_millis() as u64;
        sys::kill(child);
        (load.join().expect("load thread"), exit_ms)
    });
    let failed = r.non_2xx + r.errors;
    println!(
        "  shutdown: {failed} of {} requests in flight failed, exited {} after SIGTERM",
        r.ok + failed,
        if exit_ms >= 10_000 {
            "never (killed at 10 s)".to_string()
        } else {
            format!("{exit_ms} ms")
        }
    );
    Ok((failed, exit_ms))
}

fn contains(body: &[u8], text: &str) -> bool {
    wisp_load::find(body, text.as_bytes()).is_some()
}

fn failures(n: u64) -> String {
    if n > 0 {
        format!(", {n} failures")
    } else {
        String::new()
    }
}

/// Which user counts to try: from 500, doubling while a step passes, then
/// two bisections between the most that passed and the fewest that failed,
/// geometric (so 2^¼ ≈ 1.19× apart at the end); never past `max`.
struct Ramp {
    max: usize,
    pass: usize,
    fail: Option<usize>,
    bisections: u32,
}

impl Ramp {
    fn new(max: usize) -> Ramp {
        Ramp {
            max: max.max(1),
            pass: 0,
            fail: None,
            bisections: 0,
        }
    }

    fn first(&self) -> usize {
        START.min(self.max)
    }

    /// The step after `n` users, which passed or not; `None` when done.
    fn next(&mut self, n: usize, passed: bool) -> Option<usize> {
        if passed {
            self.pass = self.pass.max(n);
        } else {
            self.fail = Some(self.fail.map_or(n, |f| f.min(n)));
        }
        let Some(fail) = self.fail else {
            return (n < self.max).then(|| (n * 2).min(self.max));
        };
        if self.bisections == 2 {
            return None;
        }
        self.bisections += 1;
        let mid = if self.pass == 0 {
            fail / 2
        } else {
            (self.pass as f64 * fail as f64).sqrt().round() as usize
        };
        (mid > self.pass && mid < fail).then_some(mid)
    }
}

/// A figure of a test, or why there is none: "-" not run, "n/a" no route.
fn cell<T: Copy>(v: Option<Option<T>>, show: impl Fn(T) -> String) -> String {
    match v {
        None => "-".into(),
        Some(None) => "n/a".into(),
        Some(Some(v)) => show(v),
    }
}

/// A table column: its heading, the test it shows, and its cell per row.
type Column<'a> = (String, &'static str, Box<dyn Fn(&Row) -> String + 'a>);

/// `columns` of the tests that ran, as a Markdown table of `rows`.
fn table(rows: &[&Row], opt: &Options, columns: Vec<Column>) {
    let columns: Vec<Column> = columns.into_iter().filter(|c| runs(opt, c.1)).collect();
    if columns.is_empty() {
        return;
    }
    let mut lines = vec![
        std::iter::once("Server".to_string())
            .chain(columns.iter().map(|c| c.0.clone()))
            .collect::<Vec<_>>(),
    ];
    for r in rows {
        let name = if r.server == "Wisp" {
            "**Wisp**".to_string()
        } else {
            r.server.to_string()
        };
        lines.push(
            std::iter::once(name)
                .chain(columns.iter().map(|c| (c.2)(r)))
                .collect(),
        );
    }
    println!();
    print_markdown(&lines, &[0]);
}

/// The results as three Markdown tables (traffic, app work, robustness),
/// then where Wisp ranks on each figure.
pub fn print(rows: &[Row], opt: &Options) {
    let by = |f: fn(&Row) -> f64| {
        let mut order: Vec<&Row> = rows.iter().collect();
        order.sort_by(|a, b| f(b).total_cmp(&f(a)));
        order
    };
    let users = |r: &Row| r.users.map(Some);
    table(
        &by(|r| r.users.map_or(-1.0, |c| c.users as f64)),
        opt,
        vec![
            (
                format!("Users (p99 ≤ {} ms)", opt.slo_ms),
                "users",
                Box::new(move |r| {
                    cell(users(r), |c| {
                        let n = thousands(c.users as u64);
                        if c.limited {
                            format!("≥ {n} (load-limited)")
                        } else {
                            n
                        }
                    })
                }),
            ),
            (
                "p99".into(),
                "users",
                Box::new(move |r| cell(users(r), |c| ms(c.p99))),
            ),
            (
                "CPU %".into(),
                "users",
                Box::new(move |r| cell(users(r), |c| format!("{:.1}", c.cpu))),
            ),
            (
                "MB".into(),
                "users",
                Box::new(move |r| cell(users(r), |c| format!("{:.0}", c.mb))),
            ),
            (
                "KB/user".into(),
                "users",
                Box::new(move |r| cell(users(r), |c| format!("{:.1}", c.kb_per_user))),
            ),
            (
                "2×: served, p99".into(),
                "users",
                Box::new(move |r| {
                    cell(users(r).map(|c| c.and_then(|c| c.over)), |(served, p99)| {
                        format!("{served:.0}%, {}", ms(p99))
                    })
                }),
            ),
            (
                "Churn conn/s".into(),
                "churn",
                Box::new(|r| cell(r.churn.map(Some), |(c, _)| thousands(c as u64))),
            ),
            (
                "Churn µs".into(),
                "churn",
                Box::new(|r| cell(r.churn.map(Some), |(_, us)| format!("{us:.1}"))),
            ),
            (
                "Slow: kept, p99".into(),
                "slow",
                Box::new(|r| {
                    cell(r.slow.map(Some), |(kept, p99, _)| {
                        format!("{kept:.0}%, {}", ms(p99))
                    })
                }),
            ),
            (
                "Slow cut".into(),
                "slow",
                Box::new(|r| {
                    cell(r.slow.map(Some), |(_, _, cut)| {
                        format!("{cut} of {}", thousands(SLOW_CLIENTS as u64))
                    })
                }),
            ),
        ],
    );
    let rate = |v: f64| thousands(v as u64);
    table(
        &by(|r| r.echo.flatten().map_or(-1.0, |e| e.0)),
        opt,
        vec![
            (
                "Wait req/s".into(),
                "wait",
                Box::new(move |r| cell(r.wait, |w| rate(w.0))),
            ),
            (
                "Wait p50, p99".into(),
                "wait",
                Box::new(|r| cell(r.wait, |w| format!("{}, {}", ms(w.1), ms(w.2)))),
            ),
            (
                "Echo req/s".into(),
                "echo",
                Box::new(move |r| cell(r.echo, |e| rate(e.0))),
            ),
            (
                "Echo µs".into(),
                "echo",
                Box::new(|r| cell(r.echo, |e| format!("{:.1}", e.1))),
            ),
            (
                "Upload MB/s".into(),
                "upload",
                Box::new(move |r| cell(r.upload, |u| rate(u.0))),
            ),
            (
                "Upload peak MB".into(),
                "upload",
                Box::new(|r| cell(r.upload, |u| format!("{:.0}", u.1))),
            ),
            (
                "List req/s".into(),
                "list",
                Box::new(move |r| cell(r.list, |l| rate(l.0))),
            ),
            (
                "List µs".into(),
                "list",
                Box::new(|r| cell(r.list, |l| format!("{:.1}", l.1))),
            ),
            (
                "Static req/s".into(),
                "static",
                Box::new(move |r| cell(r.static_file, rate)),
            ),
            (
                "WS KB/socket".into(),
                "ws",
                Box::new(|r| cell(r.ws, |w| format!("{:.1}", w.0))),
            ),
            (
                "WS msgs/s, p99".into(),
                "ws",
                Box::new(move |r| cell(r.ws, |w| format!("{}, {}", rate(w.1), ms(w.2)))),
            ),
        ],
    );
    table(
        &by(|r| {
            let handled = r.abuse.as_ref().map_or(0, |a| a.0) as f64;
            let drift = r.soak.flatten().map_or(0.0, |(f, l)| drift(f, l));
            handled * 1000.0 - drift
        }),
        opt,
        vec![
            (
                "Abuse".into(),
                "abuse",
                Box::new(|r| {
                    cell(r.abuse.as_ref().map(|a| Some(a.0)), |handled| {
                        format!("{handled}/5")
                    })
                }),
            ),
            (
                "Abuse failures".into(),
                "abuse",
                Box::new(|r| match &r.abuse {
                    Some((_, f)) if !f.is_empty() => f.join("; "),
                    Some(_) => "none".into(),
                    None => "-".into(),
                }),
            ),
            (
                "Soak MB".into(),
                "soak",
                Box::new(|r| cell(r.soak, |(f, l)| format!("{f:.0} → {l:.0}"))),
            ),
            (
                "Soak drift".into(),
                "soak",
                Box::new(|r| {
                    cell(r.soak, |(f, l)| {
                        let grew = if grows(f, l) { " grows" } else { "" };
                        format!("{:+.0}%{grew}", drift(f, l))
                    })
                }),
            ),
            (
                "Shutdown failed".into(),
                "shutdown",
                Box::new(|r| cell(r.shutdown, |s| s.0.to_string())),
            ),
            (
                "Shutdown ms".into(),
                "shutdown",
                Box::new(|r| {
                    cell(r.shutdown, |s| {
                        if s.1 >= 10_000 {
                            "killed at 10 s".into()
                        } else {
                            thousands(s.1)
                        }
                    })
                }),
            ),
        ],
    );
    println!(
        "\nUsers each send a request every {:.1} s (±50%); the most whose p99, timed from when each request was due, stays within {} ms. n/a: no such route. -: not run.\n",
        opt.think.as_secs_f64(),
        opt.slo_ms
    );

    // Where Wisp ranks on each figure: more is better, then less.
    type Figure = fn(&Row) -> Option<f64>;
    let high: [(&str, &str, &str, Figure); 10] = [
        ("users", "real users", " users", |r| {
            r.users.map(|c| c.users as f64)
        }),
        ("churn", "real churn", " conn/s", |r| r.churn.map(|c| c.0)),
        ("slow", "real slow clients", "% kept", |r| {
            r.slow.map(|s| s.0.round())
        }),
        ("wait", "real wait", " req/s", |r| {
            r.wait.flatten().map(|w| w.0)
        }),
        ("echo", "real echo", " req/s", |r| {
            r.echo.flatten().map(|e| e.0)
        }),
        ("upload", "real upload", " MB/s", |r| {
            r.upload.flatten().map(|u| u.0)
        }),
        ("list", "real list", " req/s", |r| {
            r.list.flatten().map(|l| l.0)
        }),
        ("static", "real static", " req/s", |r| {
            r.static_file.flatten()
        }),
        ("ws", "real ws echo", " msgs/s", |r| {
            r.ws.flatten().map(|w| w.1)
        }),
        ("abuse", "real abuse handled", " of 5", |r| {
            r.abuse.as_ref().map(|a| a.0 as f64)
        }),
    ];
    for (test, tag, unit, f) in high {
        if runs(opt, test) {
            let mut all: Vec<(&str, f64)> = rows
                .iter()
                .filter_map(|r| Some((r.server, f(r)?)))
                .collect();
            all.sort_by(|a, b| b.1.total_cmp(&a.1));
            rank_high(tag, &all, unit);
        }
    }
    let low: [(&str, &str, &str, Figure); 10] = [
        ("users", "real memory per user", "KB", |r| {
            r.users.filter(|c| c.users > 0).map(|c| c.kb_per_user)
        }),
        ("churn", "real churn CPU per connection", "µs", |r| {
            r.churn.map(|c| c.1)
        }),
        ("wait", "real wait p99", "ms", |r| {
            r.wait.flatten().map(|w| w.2 as f64 / 1000.0)
        }),
        ("echo", "real echo CPU per request", "µs", |r| {
            r.echo.flatten().map(|e| e.1)
        }),
        ("upload", "real upload peak memory", "MB", |r| {
            r.upload.flatten().map(|u| u.1)
        }),
        ("list", "real list CPU per request", "µs", |r| {
            r.list.flatten().map(|l| l.1)
        }),
        ("ws", "real memory per WebSocket", "KB", |r| {
            r.ws.flatten().map(|w| w.0)
        }),
        ("soak", "real soak drift", "%", |r| {
            r.soak.flatten().map(|(f, l)| drift(f, l))
        }),
        ("shutdown", "real shutdown failures", "", |r| {
            r.shutdown.flatten().map(|s| s.0 as f64)
        }),
        ("shutdown", "real shutdown time", "ms", |r| {
            r.shutdown.flatten().map(|s| s.1 as f64)
        }),
    ];
    for (test, tag, unit, f) in low {
        if runs(opt, test) {
            let all = rows
                .iter()
                .filter_map(|r| Some((r.server, f(r)?)))
                .collect();
            rank_low(tag, all, |v| {
                format!("{v:.1} {unit}").trim_end().to_string()
            });
        }
    }
}

/// Appends a line per server and figure to `file` (server,test,figure,
/// value), with a header when it is new: every test's figures fit.
pub fn write_csv(file: &Path, rows: &[Row]) {
    let mut text = String::new();
    if !file.exists() {
        text.push_str("server,test,figure,value\n");
    }
    for r in rows {
        let mut put = |test: &str, figure: &str, v: f64| {
            text.push_str(&format!("{},{test},{figure},{v}\n", r.server));
        };
        if let Some(c) = r.users {
            put("users", "users", c.users as f64);
            put("users", "load-limited", f64::from(u8::from(c.limited)));
            put("users", "p99 us", c.p99 as f64);
            put("users", "cpu %", c.cpu);
            put("users", "MB", c.mb);
            put("users", "KB per user", c.kb_per_user);
            if let Some((served, p99)) = c.over {
                put("users", "2x served %", served);
                put("users", "2x p99 us", p99 as f64);
            }
        }
        if let Some((rps, us)) = r.churn {
            put("churn", "conn/s", rps);
            put("churn", "cpu us", us);
        }
        if let Some((kept, p99, cut)) = r.slow {
            put("slow", "kept %", kept);
            put("slow", "p99 us", p99 as f64);
            put("slow", "cut", cut as f64);
        }
        if let Some((rps, p50, p99)) = r.wait.flatten() {
            put("wait", "req/s", rps);
            put("wait", "p50 us", p50 as f64);
            put("wait", "p99 us", p99 as f64);
        }
        for (test, v) in [("echo", r.echo), ("list", r.list)] {
            if let Some((rps, us)) = v.flatten() {
                put(test, "req/s", rps);
                put(test, "cpu us", us);
            }
        }
        if let Some((mbs, peak)) = r.upload.flatten() {
            put("upload", "MB/s", mbs);
            put("upload", "peak MB", peak);
        }
        if let Some(rps) = r.static_file.flatten() {
            put("static", "req/s", rps);
        }
        if let Some((kb, msgs, p99)) = r.ws.flatten() {
            put("ws", "KB per socket", kb);
            put("ws", "msgs/s", msgs);
            put("ws", "p99 us", p99 as f64);
        }
        if let Some((handled, _)) = &r.abuse {
            put("abuse", "handled", *handled as f64);
        }
        if let Some((first, last)) = r.soak.flatten() {
            put("soak", "first MB", first);
            put("soak", "last MB", last);
        }
        if let Some((failed, exit_ms)) = r.shutdown.flatten() {
            put("shutdown", "failed", failed as f64);
            put("shutdown", "exit ms", exit_ms as f64);
        }
    }
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(file)
        .and_then(|mut f| f.write_all(text.as_bytes()))
        .unwrap_or_else(|e| die(&format!("{}: {e}", file.display())));
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The counts tried when steps pass up to `limit` users.
    fn tried(max: usize, limit: usize) -> Vec<usize> {
        let mut ramp = Ramp::new(max);
        let mut tried = vec![ramp.first()];
        while let Some(n) = ramp.next(tried[tried.len() - 1], tried[tried.len() - 1] <= limit) {
            tried.push(n);
        }
        tried
    }

    #[test]
    fn ramp_doubles_then_bisects() {
        // 16,000 fails: √(8,000 × 16,000), then √(8,000 × 11,314).
        assert_eq!(
            tried(50_000, 10_000),
            [500, 1000, 2000, 4000, 8000, 16000, 11314, 9514]
        );
        // 11,314 passes this time: √(11,314 × 16,000).
        assert_eq!(
            tried(50_000, 12_000),
            [500, 1000, 2000, 4000, 8000, 16000, 11314, 13455]
        );
        // Never past the cap, and done when the cap passes.
        assert_eq!(tried(3000, 1_000_000), [500, 1000, 2000, 3000]);
        assert_eq!(tried(3000, 2500), [500, 1000, 2000, 3000, 2449, 2711]);
        // Nothing passes: halving from the first.
        assert_eq!(tried(50_000, 0), [500, 250, 125]);
    }

    #[test]
    fn abuse_rules() {
        let addr: SocketAddr = ([127, 0, 0, 1], 3000).into();
        let cases = abuse_cases(addr);
        assert_eq!(cases.len(), 5);
        let judge = |name: &str, a: Answer| cases.iter().find(|c| c.0 == name).unwrap().2(&a);
        assert!(judge("no host", Answer::Status(400)));
        assert!(judge("no host", Answer::Closed));
        assert!(!judge("no host", Answer::Status(200)));
        assert!(!judge("1 MB header", Answer::Silent));
        assert!(judge("1 MB header", Answer::Status(431)));
        assert!(judge("64 KB header", Answer::Status(200)));
        assert!(!judge("64 KB header", Answer::Status(500)));
        assert!(!judge("bad chunked body", Answer::Status(500)));
    }

    #[test]
    fn soak_growth() {
        assert!(!grows(100.0, 120.0));
        assert!(grows(100.0, 121.0));
        assert_eq!(drift(200.0, 150.0), -25.0);
        assert_eq!(cell(Some(None::<u8>), |v| v.to_string()), "n/a");
        assert_eq!(cell(None::<Option<u8>>, |v| v.to_string()), "-");
        assert_eq!(cell(Some(Some(7)), |v| v.to_string()), "7");
    }
}
