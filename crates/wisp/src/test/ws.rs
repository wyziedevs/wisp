//! The test client's WebSocket: the app's handler runs on the server's own
//! codec over a loopback socket, and the test speaks frames to it.

use crate::Message;
use crate::ws::Upgrade;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::time::Duration;

/// How long `recv` waits for the server before it gives up.
const WAIT: Duration = Duration::from_secs(5);

/// An open WebSocket from [`Client::websocket`](super::Client::websocket).
/// Dropping it closes the connection and ends the handler.
pub struct TestSocket {
    stream: TcpStream,
}

impl TestSocket {
    pub(super) fn open(upgrade: Upgrade, path: &str) -> TestSocket {
        let listener = TcpListener::bind("127.0.0.1:0").expect("a loopback port");
        let addr = listener.local_addr().expect("its address");
        let stream = TcpStream::connect(addr).expect("a loopback connection");
        let (server, _) = listener.accept().expect("the server end");
        server.set_nonblocking(true).expect("a non-blocking socket");
        let path = path.to_string();
        // The handler runs apart from the test's own runtime, which only
        // runs while the test is inside a request.
        std::thread::spawn(move || {
            let Ok(rt) = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            else {
                return;
            };
            rt.block_on(async {
                if let Ok(tcp) = tokio::net::TcpStream::from_std(server) {
                    crate::ws::serve(tcp, Vec::new(), 1 << 20, upgrade, &path).await;
                }
            });
        });
        stream.set_read_timeout(Some(WAIT)).expect("a read timeout");
        TestSocket { stream }
    }

    /// Sends a message: a `String` or `&str` as text, bytes as binary.
    pub fn send(&mut self, msg: impl Into<Message>) {
        match msg.into() {
            Message::Text(t) => self.write(1, t.as_bytes()),
            Message::Binary(b) => self.write(2, &b),
        }
    }

    /// The next message, or `None` once the server has closed the socket
    /// (or says nothing for 5 seconds). Pings are answered meanwhile.
    pub fn recv(&mut self) -> Option<Message> {
        loop {
            let (op, data) = self.read()?;
            match op {
                1 => return String::from_utf8(data).ok().map(Message::Text),
                2 => return Some(Message::Binary(data)),
                9 => self.write(10, &data),
                8 => {
                    self.write(8, &data);
                    return None;
                }
                _ => {}
            }
        }
    }

    /// Sends a close, as a browser's `ws.close()` does.
    pub fn close(&mut self) {
        self.write(8, &1000u16.to_be_bytes());
    }

    /// One frame as a client sends it: masked.
    fn write(&mut self, op: u8, payload: &[u8]) {
        let mask = [0x37, 0xfa, 0x21, 0x3d];
        let mut f = vec![0x80 | op];
        match payload.len() {
            n @ 0..126 => f.push(0x80 | n as u8),
            n @ 126..=0xffff => {
                f.push(0x80 | 126);
                f.extend_from_slice(&(n as u16).to_be_bytes());
            }
            n => {
                f.push(0x80 | 127);
                f.extend_from_slice(&(n as u64).to_be_bytes());
            }
        }
        f.extend_from_slice(&mask);
        f.extend(payload.iter().enumerate().map(|(i, b)| b ^ mask[i & 3]));
        // A server that already closed is `recv`'s `None`, not a panic here.
        let _ = self.stream.write_all(&f);
    }

    /// One frame (the server sends no fragments).
    fn read(&mut self) -> Option<(u8, Vec<u8>)> {
        let mut head = [0u8; 2];
        self.stream.read_exact(&mut head).ok()?;
        let len = match head[1] & 0x7f {
            126 => {
                let mut b = [0u8; 2];
                self.stream.read_exact(&mut b).ok()?;
                u16::from_be_bytes(b) as usize
            }
            127 => {
                let mut b = [0u8; 8];
                self.stream.read_exact(&mut b).ok()?;
                usize::try_from(u64::from_be_bytes(b)).ok()?
            }
            n => n as usize,
        };
        let mut data = vec![0; len];
        self.stream.read_exact(&mut data).ok()?;
        Some((head[0] & 0x0f, data))
    }
}
