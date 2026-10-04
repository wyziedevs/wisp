//! Server-Sent Events for browsers in dev.
//!
//! Lives in the CLI, not the app, so browsers stay connected while the app
//! is rebuilt and restarted. A thread per client is fine: it is a handful of
//! tabs on localhost.
//!
//! Any website can ask a loopback port for something, so only the app's own
//! pages may read the stream (it carries compile errors): the origins of the
//! app's address, and the ones in `$WISP_DEV_ORIGIN` (comma-separated, for a
//! proxy or a tunnel in front of the app).

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

pub struct Events {
    pub port: u16,
    clients: Arc<Mutex<Vec<TcpStream>>>,
    origins: Arc<Mutex<Vec<String>>>,
    state: Arc<Mutex<State>>,
}

/// What a browser that connects late, or reconnects, needs to catch up.
#[derive(Default)]
struct State {
    /// How many updates (reload, hot, full, css) have been sent: a tab that
    /// was away (asleep, or reconnecting) sees it differs and reloads.
    updates: u64,
    /// The frames of the build error on show, sent again to every new
    /// connection: a page loaded after the failed build shows it too.
    error: String,
}

impl Events {
    /// `app_port` is the app's, whose pages on loopback may read the stream.
    pub fn start(app_port: u16) -> std::io::Result<Events> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let port = listener.local_addr()?.port();
        let clients: Arc<Mutex<Vec<TcpStream>>> = Arc::default();
        let mut allowed = Vec::new();
        if let Ok(extra) = std::env::var("WISP_DEV_ORIGIN") {
            let listed = extra.split(',').map(str::trim).filter(|o| !o.is_empty());
            allowed.extend(listed.map(String::from));
        }
        let origins = Arc::new(Mutex::new(allowed));
        let events = Events {
            port,
            clients: clients.clone(),
            origins,
            state: Arc::default(),
        };
        events.allow(SocketAddr::from(([127, 0, 0, 1], app_port)));
        let list = clients.clone();
        let origins = events.origins.clone();
        let state = events.state.clone();
        thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                // Off the accept thread, so a connection that never sends its
                // request cannot keep the next one waiting.
                let list = list.clone();
                let origins = origins.clone();
                let state = state.clone();
                thread::spawn(move || {
                    let allowed = origins.lock().unwrap_or_else(|e| e.into_inner()).clone();
                    if let Some(mut s) = accept(stream, &allowed) {
                        // Under the list's lock, so no update falls between
                        // what it is told and the ones that follow.
                        let mut list = list.lock().unwrap_or_else(|e| e.into_inner());
                        let state = state.lock().unwrap_or_else(|e| e.into_inner());
                        let hello = frame("hello", &state.updates.to_string()) + &state.error;
                        if s.write_all(hello.as_bytes()).is_ok() {
                            list.push(s);
                        }
                    }
                });
            }
        });
        Ok(events)
    }

    /// Lets the pages of an app at `addr` read the stream: by that address,
    /// and by every loopback name for its port.
    pub fn allow(&self, addr: SocketAddr) {
        let port = addr.port();
        let mut origins = self.origins.lock().unwrap_or_else(|e| e.into_inner());
        for origin in [
            format!("http://{addr}"),
            format!("http://localhost:{port}"),
            format!("http://127.0.0.1:{port}"),
            format!("http://[::1]:{port}"),
        ] {
            if !origins.contains(&origin) {
                origins.push(origin);
            }
        }
    }

    /// Sends one event. `data` may span lines; the browser gets `kind\ndata`.
    /// A client that takes too long to take it is dropped, since the file
    /// watcher waits on this.
    pub fn send(&self, kind: &str, data: &str) {
        let msg = frame(kind, data);
        let mut clients = self.clients.lock().unwrap_or_else(|e| e.into_inner());
        {
            let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            match kind {
                "title" => state.error = msg.clone(),
                "error" => state.error.push_str(&msg),
                "building" => {}
                _ => {
                    state.error.clear();
                    state.updates += 1;
                }
            }
        }
        clients.retain_mut(|c| c.write_all(msg.as_bytes()).is_ok());
    }
}

/// One event: `kind`, then `data`, a line each.
fn frame(kind: &str, data: &str) -> String {
    let mut msg = format!("data: {kind}\n");
    for line in data.lines() {
        msg.push_str("data: ");
        msg.push_str(line);
        msg.push('\n');
    }
    msg.push('\n');
    msg
}

fn accept(mut s: TcpStream, allowed: &[String]) -> Option<TcpStream> {
    s.set_read_timeout(Some(Duration::from_secs(5))).ok()?;
    s.set_write_timeout(Some(Duration::from_secs(1))).ok()?;
    // Read the request head; any path is the event stream.
    let mut buf = [0u8; 4096];
    let mut n = 0;
    while !buf[..n].windows(4).any(|w| w == b"\r\n\r\n") {
        let got = s.read(&mut buf[n..]).ok()?;
        if got == 0 || n + got == buf.len() {
            return None;
        }
        n += got;
    }
    let head = match reply(&String::from_utf8_lossy(&buf[..n]), allowed) {
        Some(head) => head,
        None => {
            let _ = s.write_all(b"HTTP/1.1 403 Forbidden\r\ncontent-length: 0\r\n\r\n");
            return None;
        }
    };
    s.write_all(head.as_bytes()).ok()?;
    Some(s)
}

/// The answer to a request head, or `None` for a page of another origin. A
/// request without an `Origin` is not a page's: a tool, or the page itself
/// loading the stream as a same-origin request; it gets no CORS header.
fn reply(request: &str, allowed: &[String]) -> Option<String> {
    let origin = request.lines().find_map(|line| {
        let (name, value) = line.split_once(':')?;
        name.eq_ignore_ascii_case("origin").then(|| value.trim())
    });
    let cors = match origin {
        Some(o) if allowed.iter().any(|a| a == o) => {
            format!("access-control-allow-origin: {o}\r\nvary: origin\r\n")
        }
        Some(_) => return None,
        None => String::new(),
    };
    Some(format!(
        "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncache-control: no-store\r\n{cors}\r\nretry: 500\n\n"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn allowed() -> Vec<String> {
        let e = Events {
            port: 0,
            clients: Arc::default(),
            origins: Arc::default(),
            state: Arc::default(),
        };
        e.allow(SocketAddr::from(([127, 0, 0, 1], 3000)));
        e.origins.lock().unwrap().clone()
    }

    fn get(origin: &str) -> Option<String> {
        reply(
            &format!("GET / HTTP/1.1\r\nhost: x\r\n{origin}\r\n\r\n"),
            &allowed(),
        )
    }

    /// Reads from the stream until `needle` has come.
    fn read_until(c: &mut TcpStream, needle: &str) -> String {
        let mut got = String::new();
        let mut buf = [0u8; 1024];
        while !got.contains(needle) {
            let n = c.read(&mut buf).expect("the stream stays open");
            assert!(n > 0, "closed before {needle:?}: {got:?}");
            got.push_str(&String::from_utf8_lossy(&buf[..n]));
        }
        got
    }

    #[test]
    fn a_late_tab_hears_the_error_on_show_and_how_many_updates_it_missed() {
        let e = Events::start(3000).unwrap();
        e.send("reload", "");
        e.send("title", "Old\nOld\nOld");
        e.send("error", "old");
        e.send("title", "Build Failed\nSummary\nCompiler Output");
        e.send("error", "boom");
        let mut c = TcpStream::connect(("127.0.0.1", e.port)).unwrap();
        c.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        c.write_all(b"GET /events HTTP/1.1\r\nhost: x\r\n\r\n")
            .unwrap();
        let got = read_until(&mut c, "data: error\ndata: boom\n\n");
        assert!(got.contains("data: hello\ndata: 1\n\n"), "{got:?}");
        assert!(got.contains("data: title\ndata: Build Failed\n"), "{got:?}");
        assert!(!got.contains("old"), "only the error on show: {got:?}");
        // A fix ends it: the next tab sees the update count, and no error.
        e.send("reload", "");
        read_until(&mut c, "data: reload\n\n");
        let mut d = TcpStream::connect(("127.0.0.1", e.port)).unwrap();
        d.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        d.write_all(b"GET /events HTTP/1.1\r\nhost: x\r\n\r\n")
            .unwrap();
        let got = read_until(&mut d, "data: hello\ndata: 2\n\n");
        assert!(!got.contains("boom"), "{got:?}");
    }

    #[test]
    fn only_the_apps_own_origins_read_the_stream() {
        for ok in [
            "http://localhost:3000",
            "http://127.0.0.1:3000",
            "http://[::1]:3000",
        ] {
            let head = get(&format!("Origin: {ok}")).unwrap();
            assert!(head.contains(&format!("access-control-allow-origin: {ok}\r\n")));
            assert!(!head.contains("origin: *"));
        }
        for bad in [
            "https://evil.example",
            "http://localhost:3001",
            "http://localhost:3000.evil.example",
            "null",
        ] {
            assert_eq!(get(&format!("origin: {bad}")), None, "{bad}");
        }
        let plain = get("x-a: b").unwrap();
        assert!(plain.starts_with("HTTP/1.1 200") && !plain.contains("access-control"));
    }
}
