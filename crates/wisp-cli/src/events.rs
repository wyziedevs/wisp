//! Server-Sent Events for browsers in dev.
//!
//! Lives in the CLI, not the app, so browsers stay connected while the app
//! is rebuilt and restarted. A thread per client is fine: it is a handful of
//! tabs on localhost.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

pub struct Events {
    pub port: u16,
    clients: Arc<Mutex<Vec<TcpStream>>>,
}

impl Events {
    pub fn start() -> std::io::Result<Events> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let port = listener.local_addr()?.port();
        let clients: Arc<Mutex<Vec<TcpStream>>> = Arc::default();
        let list = clients.clone();
        thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                // Off the accept thread, so a connection that never sends its
                // request cannot keep the next one waiting.
                let list = list.clone();
                thread::spawn(move || {
                    if let Some(s) = accept(stream) {
                        list.lock().unwrap_or_else(|e| e.into_inner()).push(s);
                    }
                });
            }
        });
        Ok(Events { port, clients })
    }

    /// Sends one event. `data` may span lines; the browser gets `kind\ndata`.
    /// A client that takes too long to take it is dropped, since the file
    /// watcher waits on this.
    pub fn send(&self, kind: &str, data: &str) {
        let mut msg = format!("data: {kind}\n");
        for line in data.lines() {
            msg.push_str("data: ");
            msg.push_str(line);
            msg.push('\n');
        }
        msg.push('\n');
        let mut clients = self.clients.lock().unwrap_or_else(|e| e.into_inner());
        clients.retain_mut(|c| c.write_all(msg.as_bytes()).is_ok());
    }
}

fn accept(mut s: TcpStream) -> Option<TcpStream> {
    s.set_read_timeout(Some(Duration::from_secs(5))).ok()?;
    s.set_write_timeout(Some(Duration::from_secs(1))).ok()?;
    // Read (and ignore) the request head; any path is the event stream.
    let mut buf = [0u8; 4096];
    let mut n = 0;
    while !buf[..n].windows(4).any(|w| w == b"\r\n\r\n") {
        let got = s.read(&mut buf[n..]).ok()?;
        if got == 0 || n + got == buf.len() {
            return None;
        }
        n += got;
    }
    let head = "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncache-control: no-store\r\n\
                access-control-allow-origin: *\r\n\r\nretry: 500\n\n";
    s.write_all(head.as_bytes()).ok()?;
    Some(s)
}
