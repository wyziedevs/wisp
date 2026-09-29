//! Load one URL and print what came back.
//!
//!   wisp-load <http://host:port/path> [-c connections] [-d seconds] [-w warmup-seconds]

use std::net::ToSocketAddrs;
use std::time::Duration;
use wisp_load::ms;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let flag = |f: &str, default: u64| -> u64 {
        let Some(i) = args.iter().position(|a| a == f) else {
            return default;
        };
        let v = args
            .get(i + 1)
            .unwrap_or_else(|| die(&format!("{f} needs a value")));
        v.parse()
            .unwrap_or_else(|_| die(&format!("bad value for {f}: {v}")))
    };
    let url = args
        .iter()
        .find(|a| a.starts_with("http://"))
        .unwrap_or_else(|| die("usage: wisp-load <http://host:port/path> [-c 64] [-d 10] [-w 3]"));
    let connections = flag("-c", 64) as usize;
    let seconds = flag("-d", 10).max(1);
    let warmup = flag("-w", 3);

    let rest = &url["http://".len()..];
    let (host, path) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, "/"),
    };
    let addr = host
        .to_socket_addrs()
        .ok()
        .and_then(|mut a| a.next())
        .unwrap_or_else(|| die(&format!("cannot resolve {host}")));

    let r = wisp_load::run(
        addr,
        path,
        connections,
        Duration::from_secs(warmup),
        Duration::from_secs(seconds),
    );
    let h = &r.latency;
    println!("{url}  ({connections} connections, {seconds}s after {warmup}s warmup)");
    println!("  requests  {:>10}   {:>10.0} req/s", r.ok, r.rps());
    println!(
        "  latency   p50 {}  p90 {}  p99 {}  p99.9 {}  max {}",
        ms(h.percentile(0.50)),
        ms(h.percentile(0.90)),
        ms(h.percentile(0.99)),
        ms(h.percentile(0.999)),
        ms(h.max)
    );
    println!(
        "  bytes     {:>10.1} MB/s    {} per response",
        r.bytes as f64 / r.seconds / 1e6,
        r.bytes.checked_div(r.ok).unwrap_or(0)
    );
    if r.non_2xx + r.errors > 0 {
        println!(
            "  failures  {} non-2xx, {} socket errors",
            r.non_2xx, r.errors
        );
    }
}

fn die(msg: &str) -> ! {
    eprintln!("wisp-load: {msg}");
    std::process::exit(2);
}
