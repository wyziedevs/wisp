//! Closed-loop HTTP/1.1 load generator.
//!
//! One thread per connection, each sending a request and waiting for the
//! whole response before sending the next (no pipelining), like wrk and
//! bombardier do by default. Blocking sockets keep it simple; with one
//! request in flight per connection there is nothing for async to overlap.
//!
//!   wisp-load <http://host:port/path> [-c connections] [-d seconds] [-w warmup-seconds]
//!
//! Only requests that complete inside the measured window are counted.

use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};
use std::thread;
use std::time::{Duration, Instant};

const WARMUP: u8 = 0;
const MEASURE: u8 = 1;
const STOP: u8 = 2;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let flag = |f: &str, default: u64| -> u64 {
        args.iter().position(|a| a == f).and_then(|i| args.get(i + 1)).map_or(default, |v| v.parse().unwrap_or_else(|_| die(&format!("bad value for {f}: {v}"))))
    };
    let url = args.iter().find(|a| a.starts_with("http://")).unwrap_or_else(|| die("usage: wisp-load <http://host:port/path> [-c 64] [-d 10] [-w 3]"));
    let connections = flag("-c", 64).max(1);
    let seconds = flag("-d", 10).max(1);
    let warmup = flag("-w", 3);

    let (host, path) = url["http://".len()..].split_once('/').map_or((&url[7..], "/".to_string()), |(h, p)| (h, format!("/{p}")));
    let addr = host.to_socket_addrs().ok().and_then(|mut a| a.next()).unwrap_or_else(|| die(&format!("cannot resolve {host}")));
    let request: Arc<[u8]> = format!("GET {path} HTTP/1.1\r\nhost: {host}\r\naccept: text/html,*/*\r\n\r\n").into_bytes().into();

    let phase = Arc::new(AtomicU8::new(WARMUP));
    let workers: Vec<_> = (0..connections)
        .map(|_| {
            let (phase, request) = (phase.clone(), request.clone());
            thread::spawn(move || worker(addr, &request, &phase))
        })
        .collect();

    thread::sleep(Duration::from_secs(warmup));
    phase.store(MEASURE, Ordering::Release);
    let started = Instant::now();
    thread::sleep(Duration::from_secs(seconds));
    phase.store(STOP, Ordering::Release);
    let elapsed = started.elapsed().as_secs_f64();

    let mut total = Stats::default();
    for w in workers {
        total.merge(&w.join().expect("worker panicked"));
    }

    let h = &total.latency;
    println!("{url}  ({connections} connections, {seconds}s after {warmup}s warmup)");
    println!("  requests  {:>10}   {:>10.0} req/s", total.ok, total.ok as f64 / elapsed);
    println!(
        "  latency   p50 {}  p90 {}  p99 {}  p99.9 {}  max {}",
        ms(h.percentile(0.50)),
        ms(h.percentile(0.90)),
        ms(h.percentile(0.99)),
        ms(h.percentile(0.999)),
        ms(h.max)
    );
    println!("  bytes     {:>10.1} MB/s    {} per response", total.bytes as f64 / elapsed / 1e6, total.bytes.checked_div(total.ok).unwrap_or(0));
    if total.non_2xx + total.errors > 0 {
        println!("  failures  {} non-2xx, {} socket errors", total.non_2xx, total.errors);
    }
}

#[derive(Default)]
struct Stats {
    ok: u64,
    non_2xx: u64,
    errors: u64,
    bytes: u64,
    latency: Histogram,
}

impl Stats {
    fn merge(&mut self, o: &Stats) {
        self.ok += o.ok;
        self.non_2xx += o.non_2xx;
        self.errors += o.errors;
        self.bytes += o.bytes;
        self.latency.merge(&o.latency);
    }
}

fn worker(addr: std::net::SocketAddr, request: &[u8], phase: &AtomicU8) -> Stats {
    let mut stats = Stats::default();
    let mut buf = Vec::with_capacity(64 * 1024);
    let mut conn: Option<TcpStream> = None;
    loop {
        let p = phase.load(Ordering::Acquire);
        if p == STOP {
            return stats;
        }
        let s = match &mut conn {
            Some(s) => s,
            None => match TcpStream::connect(addr) {
                Ok(s) => {
                    let _ = s.set_nodelay(true);
                    let _ = s.set_read_timeout(Some(Duration::from_secs(5)));
                    buf.clear();
                    conn.insert(s)
                }
                Err(_) => {
                    stats.errors += u64::from(p == MEASURE);
                    thread::sleep(Duration::from_millis(10));
                    continue;
                }
            },
        };
        let t = Instant::now();
        let result = s.write_all(request).ok().and_then(|()| read_response(s, &mut buf));
        let took = t.elapsed();
        // A request that started during warmup but ended in the window still
        // counts: the window is defined by completions.
        if phase.load(Ordering::Acquire) != MEASURE {
            if result.is_none() {
                conn = None;
            }
            continue;
        }
        match result {
            Some(Response { status, len, close }) => {
                if (200..300).contains(&status) {
                    stats.ok += 1;
                    stats.bytes += len as u64;
                    stats.latency.record(took.as_micros() as u64);
                } else {
                    stats.non_2xx += 1;
                }
                if close {
                    conn = None;
                }
            }
            None => {
                stats.errors += 1;
                conn = None;
            }
        }
    }
}

struct Response {
    status: u16,
    /// Bytes on the wire, head included.
    len: usize,
    close: bool,
}

/// Reads one response. `buf` keeps any bytes past its end for the next call.
fn read_response(s: &mut TcpStream, buf: &mut Vec<u8>) -> Option<Response> {
    let head_end = loop {
        if let Some(i) = find(buf, b"\r\n\r\n") {
            break i + 4;
        }
        fill(s, buf)?;
    };
    let head = std::str::from_utf8(&buf[..head_end]).ok()?;
    let status: u16 = head.get(9..12)?.parse().ok()?;
    let (mut length, mut chunked, mut close) = (None, false, false);
    for line in head.split("\r\n").skip(1) {
        let Some((name, value)) = line.split_once(':') else { continue };
        let value = value.trim();
        if name.eq_ignore_ascii_case("content-length") {
            length = Some(value.parse::<usize>().ok()?);
        } else if name.eq_ignore_ascii_case("transfer-encoding") {
            chunked = value.eq_ignore_ascii_case("chunked");
        } else if name.eq_ignore_ascii_case("connection") {
            close = value.eq_ignore_ascii_case("close");
        }
    }

    let end = if chunked {
        // size CRLF data CRLF ... 0 CRLF CRLF (no trailers expected)
        let mut at = head_end;
        loop {
            let line_end = loop {
                if let Some(i) = find(&buf[at..], b"\r\n") {
                    break at + i;
                }
                fill(s, buf)?;
            };
            let size_str = std::str::from_utf8(&buf[at..line_end]).ok()?;
            let size = usize::from_str_radix(size_str.split(';').next()?.trim(), 16).ok()?;
            at = line_end + 2 + size + 2;
            while buf.len() < at {
                fill(s, buf)?;
            }
            if size == 0 {
                break at;
            }
        }
    } else {
        let end = head_end + length.unwrap_or(0);
        while buf.len() < end {
            fill(s, buf)?;
        }
        end
    };
    buf.drain(..end);
    Some(Response { status, len: end, close })
}

fn fill(s: &mut TcpStream, buf: &mut Vec<u8>) -> Option<()> {
    let len = buf.len();
    if buf.capacity() - len < 16 * 1024 {
        buf.reserve(64 * 1024);
    }
    buf.resize(buf.capacity(), 0);
    let n = s.read(&mut buf[len..]);
    match n {
        Ok(n) if n > 0 => {
            buf.truncate(len + n);
            Some(())
        }
        _ => {
            buf.truncate(len);
            None
        }
    }
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

/// Log-linear latency histogram in microseconds: exact below 64µs, then 32
/// buckets per power of two (about 3% resolution). Fixed size, no allocation
/// per sample; `BUCKETS` covers every u64.
struct Histogram {
    counts: Box<[u64; BUCKETS]>,
    total: u64,
    max: u64,
}

impl Default for Histogram {
    fn default() -> Self {
        Histogram { counts: Box::new([0; BUCKETS]), total: 0, max: 0 }
    }
}

impl Histogram {
    fn index(v: u64) -> usize {
        if v < 64 {
            return v as usize;
        }
        let shift = 63 - v.leading_zeros() as usize - 5;
        shift * 32 + (v >> shift) as usize
    }

    fn lower_bound(i: usize) -> u64 {
        if i < 64 {
            return i as u64;
        }
        let shift = i / 32 - 1;
        ((i % 32 + 32) as u64) << shift
    }

    fn record(&mut self, us: u64) {
        self.counts[Self::index(us)] += 1;
        self.total += 1;
        self.max = self.max.max(us);
    }

    fn merge(&mut self, o: &Histogram) {
        for (a, b) in self.counts.iter_mut().zip(o.counts.iter()) {
            *a += b;
        }
        self.total += o.total;
        self.max = self.max.max(o.max);
    }

    fn percentile(&self, p: f64) -> u64 {
        let target = ((self.total as f64 * p).ceil() as u64).max(1);
        let mut seen = 0;
        for (i, &c) in self.counts.iter().enumerate() {
            seen += c;
            if seen >= target {
                return Self::lower_bound(i);
            }
        }
        self.max
    }
}

const BUCKETS: usize = 58 * 32 + 64;

fn ms(us: u64) -> String {
    format!("{:.2}ms", us as f64 / 1000.0)
}

fn die(msg: &str) -> ! {
    eprintln!("wisp-load: {msg}");
    std::process::exit(2);
}

#[cfg(test)]
mod tests {
    use super::{BUCKETS, Histogram};

    #[test]
    fn histogram_buckets_are_contiguous() {
        for i in 0..BUCKETS - 1 {
            let lo = Histogram::lower_bound(i);
            assert_eq!(Histogram::index(lo), i, "lower bound of bucket {i}");
            assert_eq!(Histogram::index(Histogram::lower_bound(i + 1) - 1), i, "top of bucket {i}");
        }
        let mut h = Histogram::default();
        (1..=1000).for_each(|v| h.record(v));
        let p50 = h.percentile(0.5);
        assert!((484..=500).contains(&p50), "{p50}");
    }
}
