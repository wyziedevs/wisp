//! Closed-loop HTTP/1.1 load generator, used by `wisp-load` on its own and
//! by `bench-run` for every server it measures.
//!
//! One thread per connection, each sending a request and waiting for the
//! whole response before sending the next (no pipelining), like wrk and
//! bombardier do by default. Blocking sockets keep it simple; with one
//! request in flight per connection there is nothing for async to overlap.
//! A pipeline depth of N (TechEmpower's plaintext uses 16) sends N requests
//! back to back, then reads their N responses; a response's latency runs
//! from the first byte of its batch.
//!
//! Only requests that complete inside the measured window are counted.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};
use std::thread;
use std::time::{Duration, Instant};

const WARMUP: u8 = 0;
const MEASURE: u8 = 1;
const STOP: u8 = 2;

/// What one run measured.
#[derive(Default)]
pub struct Report {
    pub ok: u64,
    pub non_2xx: u64,
    pub errors: u64,
    /// Bytes on the wire, heads included.
    pub bytes: u64,
    pub latency: Histogram,
    /// Length of the measured window.
    pub seconds: f64,
}

impl Report {
    pub fn rps(&self) -> f64 {
        self.ok as f64 / self.seconds
    }

    fn merge(&mut self, o: &Report) {
        self.ok += o.ok;
        self.non_2xx += o.non_2xx;
        self.errors += o.errors;
        self.bytes += o.bytes;
        self.latency.merge(&o.latency);
    }
}

/// A `GET` of `path` with a host and an accept header, the smallest a
/// browser sends.
pub fn get_request(addr: SocketAddr, path: &str) -> String {
    format!("GET {path} HTTP/1.1\r\nhost: {addr}\r\naccept: text/html,*/*\r\n\r\n")
}

/// Runs `connections` closed loops of `request` (a whole request, head and
/// body) against `addr` for `warmup + measure`, counting what completes
/// during `measure`. Each loop sends `depth` requests at once and reads
/// their responses; 1 waits for every response before the next request.
pub fn run(
    addr: SocketAddr,
    request: &str,
    connections: usize,
    depth: usize,
    warmup: Duration,
    measure: Duration,
) -> Report {
    let depth = depth.max(1);
    let batch: Arc<[u8]> = request.repeat(depth).into_bytes().into();
    let phase = Arc::new(AtomicU8::new(WARMUP));
    let workers: Vec<_> = (0..connections.max(1))
        .map(|_| {
            let (phase, batch) = (phase.clone(), batch.clone());
            thread::spawn(move || connection(addr, &batch, depth, &phase))
        })
        .collect();

    thread::sleep(warmup);
    phase.store(MEASURE, Ordering::Release);
    let started = Instant::now();
    thread::sleep(measure);
    phase.store(STOP, Ordering::Release);
    let seconds = started.elapsed().as_secs_f64();

    let mut total = Report {
        seconds,
        ..Report::default()
    };
    for w in workers {
        total.merge(&w.join().expect("load thread panicked"));
    }
    total
}

fn connection(addr: SocketAddr, batch: &[u8], depth: usize, phase: &AtomicU8) -> Report {
    let mut report = Report::default();
    let mut buf = Vec::with_capacity(64 * 1024);
    let mut conn: Option<TcpStream> = None;
    loop {
        let p = phase.load(Ordering::Acquire);
        if p == STOP {
            return report;
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
                    report.errors += u64::from(p == MEASURE);
                    thread::sleep(Duration::from_millis(10));
                    continue;
                }
            },
        };
        let t = Instant::now();
        let mut alive = s.write_all(batch).is_ok();
        for _ in 0..depth {
            let response = if alive {
                read_response(s, &mut buf)
            } else {
                None
            };
            let took = t.elapsed();
            // A request that started during warmup but ended in the window
            // still counts: the window is defined by completions.
            let measuring = phase.load(Ordering::Acquire) == MEASURE;
            let Some(r) = response else {
                alive = false;
                report.errors += u64::from(measuring);
                continue;
            };
            // Whatever a closing server has not answered yet is lost.
            alive &= !r.close;
            if !measuring {
                continue;
            }
            if (200..300).contains(&r.status) {
                report.ok += 1;
                report.bytes += r.len as u64;
                report.latency.record(took.as_micros() as u64);
            } else {
                report.non_2xx += 1;
            }
        }
        if !alive {
            conn = None;
        }
    }
}

/// One `GET` on a connection of its own: see [`send`].
pub fn get(addr: SocketAddr, path: &str, timeout: Duration) -> Option<(u16, Vec<u8>)> {
    send(addr, &get_request(addr, path), timeout)
}

/// One request on a connection of its own: the status and the body, with
/// chunked encoding undone. For checking what a server sends, and whether
/// it is up yet: the connect gives up after 25 ms, since a refused connect
/// takes 2 s to fail on Windows.
pub fn send(addr: SocketAddr, request: &str, timeout: Duration) -> Option<(u16, Vec<u8>)> {
    let mut s = TcpStream::connect_timeout(&addr, Duration::from_millis(25)).ok()?;
    s.set_read_timeout(Some(timeout)).ok()?;
    s.write_all(request.as_bytes()).ok()?;
    let mut buf = Vec::new();
    let r = read_response(&mut s, &mut buf)?;
    Some((r.status, r.body))
}

struct Response {
    status: u16,
    /// Bytes on the wire, head included.
    len: usize,
    close: bool,
    body: Vec<u8>,
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
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        let value = value.trim();
        if name.eq_ignore_ascii_case("content-length") {
            length = Some(value.parse::<usize>().ok()?);
        } else if name.eq_ignore_ascii_case("transfer-encoding") {
            chunked = value.eq_ignore_ascii_case("chunked");
        } else if name.eq_ignore_ascii_case("connection") {
            close = value.eq_ignore_ascii_case("close");
        }
    }

    let mut body = Vec::new();
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
            body.extend_from_slice(&buf[line_end + 2..line_end + 2 + size]);
            if size == 0 {
                break at;
            }
        }
    } else {
        let end = head_end + length.unwrap_or(0);
        while buf.len() < end {
            fill(s, buf)?;
        }
        body.extend_from_slice(&buf[head_end..end]);
        end
    };
    buf.drain(..end);
    Some(Response {
        status,
        len: end,
        close,
        body,
    })
}

/// Reads into a block of this thread's, zeroed once, then appends what came:
/// zeroing `buf`'s spare room for every read cost a 64 KB memset per
/// response, on the cores the load shares.
fn fill(s: &mut TcpStream, buf: &mut Vec<u8>) -> Option<()> {
    thread_local! {
        static BLOCK: std::cell::RefCell<Vec<u8>> = std::cell::RefCell::new(vec![0; 64 * 1024]);
    }
    BLOCK.with_borrow_mut(|block| match s.read(block) {
        Ok(n) if n > 0 => {
            buf.extend_from_slice(&block[..n]);
            Some(())
        }
        _ => None,
    })
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

/// Log-linear latency histogram in microseconds: exact below 64µs, then 32
/// buckets per power of two (about 3% resolution). Fixed size, no allocation
/// per sample; `BUCKETS` covers every u64.
pub struct Histogram {
    counts: Box<[u64; BUCKETS]>,
    total: u64,
    pub max: u64,
}

impl Default for Histogram {
    fn default() -> Self {
        Histogram {
            counts: Box::new([0; BUCKETS]),
            total: 0,
            max: 0,
        }
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

    /// The latency at or under which `p` of the requests finished, in µs.
    pub fn percentile(&self, p: f64) -> u64 {
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

/// `1234` µs → `1.23ms`.
pub fn ms(us: u64) -> String {
    format!("{:.2}ms", us as f64 / 1000.0)
}

#[cfg(test)]
mod tests {
    use super::{BUCKETS, Histogram, find, get_request, run};
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::time::Duration;

    #[test]
    fn pipeline_counts_every_response() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        // Answers each request as it arrives, however many came in one read.
        std::thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            let (mut buf, mut chunk) = (Vec::new(), [0; 4096]);
            while let Ok(n @ 1..) = s.read(&mut chunk) {
                buf.extend_from_slice(&chunk[..n]);
                while let Some(i) = find(&buf, b"\r\n\r\n") {
                    buf.drain(..i + 4);
                    let _ = s.write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 2\r\n\r\nok");
                }
            }
        });
        let request = get_request(addr, "/");
        let r = run(
            addr,
            &request,
            1,
            8,
            Duration::ZERO,
            Duration::from_millis(200),
        );
        assert!(r.ok >= 8, "{}", r.ok);
        assert_eq!((r.non_2xx, r.errors), (0, 0));
        assert_eq!(r.bytes, r.ok * 40);
    }

    #[test]
    fn histogram_buckets_are_contiguous() {
        for i in 0..BUCKETS - 1 {
            let lo = Histogram::lower_bound(i);
            assert_eq!(Histogram::index(lo), i, "lower bound of bucket {i}");
            assert_eq!(
                Histogram::index(Histogram::lower_bound(i + 1) - 1),
                i,
                "top of bucket {i}"
            );
        }
        let mut h = Histogram::default();
        (1..=1000).for_each(|v| h.record(v));
        let p50 = h.percentile(0.5);
        assert!((484..=500).contains(&p50), "{p50}");
    }
}
