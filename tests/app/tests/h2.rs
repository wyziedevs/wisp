//! HTTP/2 with prior knowledge on the wire (`--features h2`): raw frames,
//! and curl where it is installed. HTTP/1 on the same port is unchanged.
#![cfg(feature = "h2")]

mod common;

use common::*;
use std::io::{Read, Write};

fn frame(kind: u8, flags: u8, id: u32, p: &[u8]) -> Vec<u8> {
    let mut f = (p.len() as u32).to_be_bytes()[1..].to_vec();
    f.extend_from_slice(&[kind, flags]);
    f.extend_from_slice(&id.to_be_bytes());
    f.extend_from_slice(p);
    f
}

/// A GET for `path` as an HPACK block, literals only.
fn get(path: &str) -> Vec<u8> {
    let mut b = vec![0x82, 0x86, 0x04, path.len() as u8];
    b.extend_from_slice(path.as_bytes());
    b.extend_from_slice(&[0x01, 9]);
    b.extend_from_slice(b"127.0.0.1");
    b
}

/// The first byte of the status field and the body of stream 1, asked on
/// a fresh connection.
fn h2_get(port: u16, path: &str) -> (u8, Vec<u8>) {
    let mut c = connect(port);
    let mut out = b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n".to_vec();
    out.extend(frame(4, 0, 0, &[]));
    out.extend(frame(1, 0x5, 1, &get(path)));
    c.write_all(&out).unwrap();
    let (mut buf, mut status, mut body) = (Vec::new(), 0, Vec::new());
    let mut chunk = [0; 4096];
    loop {
        while buf.len() >= 9 {
            let len = (buf[0] as usize) << 16 | (buf[1] as usize) << 8 | buf[2] as usize;
            if buf.len() < 9 + len {
                break;
            }
            let (kind, flags) = (buf[3], buf[4]);
            let p = buf[9..9 + len].to_vec();
            buf.drain(..9 + len);
            match kind {
                1 => status = p[0],
                0 => body.extend_from_slice(&p),
                7 => panic!("goaway {p:?}"),
                _ => {}
            }
            if kind <= 1 && flags & 1 != 0 {
                return (status, body);
            }
        }
        let n = c.read(&mut chunk).unwrap();
        assert!(n > 0, "closed");
        buf.extend_from_slice(&chunk[..n]);
    }
}

#[test]
fn h2_prior_knowledge_serves_pages() {
    for env in [&[][..], &[("WISP_IO", "epoll")][..]] {
        let server = start(env);
        let (s, body) = h2_get(server.port, "/");
        assert_eq!(s, 0x88, "{env:?}: :status 200, indexed");
        assert!(String::from_utf8_lossy(&body).contains("hello from init"));
        let (s, _) = h2_get(server.port, "/no-such-page");
        assert_eq!(s, 0x8d, "{env:?}: :status 404, indexed");
        let one = server.request("GET", "/", "", b"");
        assert_eq!(status(&one), 200, "{env:?}: HTTP/1 as before");
    }
}

/// Reads frames until `until` says so: each is (kind, flags, stream, payload).
fn frames_until(
    c: &mut std::net::TcpStream,
    buf: &mut Vec<u8>,
    mut until: impl FnMut(u8, u8, u32, &[u8]) -> bool,
) {
    let mut chunk = [0; 4096];
    loop {
        while buf.len() >= 9 {
            let len = (buf[0] as usize) << 16 | (buf[1] as usize) << 8 | buf[2] as usize;
            if buf.len() < 9 + len {
                break;
            }
            let (kind, flags) = (buf[3], buf[4]);
            let id = u32::from_be_bytes([buf[5], buf[6], buf[7], buf[8]]);
            let p = buf[9..9 + len].to_vec();
            buf.drain(..9 + len);
            assert_ne!(kind, 7, "goaway {p:?}");
            if until(kind, flags, id, &p) {
                return;
            }
        }
        let n = c.read(&mut chunk).unwrap();
        assert!(n > 0, "closed");
        buf.extend_from_slice(&chunk[..n]);
    }
}

#[test]
fn h2_streams_go_at_once() {
    for env in [&[][..], &[("WISP_IO", "epoll")][..]] {
        let server = start(env);
        let mut c = connect(server.port);
        c.set_read_timeout(Some(std::time::Duration::from_secs(5)))
            .unwrap();
        let mut out = b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n".to_vec();
        out.extend(frame(4, 0, 0, &[]));
        out.extend(frame(1, 0x5, 1, &get("/ticker")));
        c.write_all(&out).unwrap();
        let mut buf = Vec::new();
        // The event stream is open: a tick came, no end.
        frames_until(&mut c, &mut buf, |kind, flags, id, p| {
            assert!(!(id == 1 && flags & 1 != 0), "{env:?}: the ticker ended");
            kind == 0 && id == 1 && p.starts_with(b"data: tick")
        });
        // A GET beside it finishes while it stays open.
        c.write_all(&frame(1, 0x5, 3, &get("/"))).unwrap();
        let mut body = Vec::new();
        frames_until(&mut c, &mut buf, |kind, flags, id, p| {
            assert!(!(id == 1 && flags & 1 != 0), "{env:?}: the ticker ended");
            if kind == 0 && id == 3 {
                body.extend_from_slice(p);
            }
            id == 3 && flags & 1 != 0
        });
        assert!(String::from_utf8_lossy(&body).contains("hello from init"));
        // A reset ends the ticker, and the connection serves on.
        c.write_all(&frame(3, 0, 1, &8u32.to_be_bytes())).unwrap();
        c.write_all(&frame(6, 0, 0, &[7; 8])).unwrap();
        frames_until(&mut c, &mut buf, |kind, flags, _, _| {
            kind == 6 && flags & 1 != 0
        });
        c.write_all(&frame(1, 0x5, 5, &get("/"))).unwrap();
        let mut late = 0;
        frames_until(&mut c, &mut buf, |kind, flags, id, _| {
            late += (kind == 0 && id == 1) as u32;
            id == 5 && flags & 1 != 0
        });
        assert!(late <= 1, "{env:?}: the ticker went on after its reset");
    }
}

/// A client whose window is small: the answer stalls, and resumes as it
/// grants more.
#[test]
fn h2_flow_control_stalls_and_resumes() {
    let server = start(&[]);
    let mut c = connect(server.port);
    c.set_read_timeout(Some(std::time::Duration::from_secs(5)))
        .unwrap();
    let mut out = b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n".to_vec();
    // A 16-byte initial stream window.
    out.extend(frame(4, 0, 0, &[0, 4, 0, 0, 0, 16]));
    out.extend(frame(1, 0x5, 1, &get("/")));
    c.write_all(&out).unwrap();
    let (mut buf, mut body) = (Vec::new(), Vec::new());
    frames_until(&mut c, &mut buf, |kind, _, id, p| {
        if kind == 0 && id == 1 {
            body.extend_from_slice(p);
        }
        body.len() == 16
    });
    // Nothing more comes until the window grows.
    c.set_read_timeout(Some(std::time::Duration::from_millis(200)))
        .unwrap();
    let mut chunk = [0; 4096];
    if let Ok(n) = c.read(&mut chunk) {
        buf.extend_from_slice(&chunk[..n]);
    }
    let mut b = &buf[..];
    while b.len() >= 9 {
        assert_ne!(b[3], 0, "DATA past the window");
        let len = (b[0] as usize) << 16 | (b[1] as usize) << 8 | b[2] as usize;
        b = &b[(9 + len).min(b.len())..];
    }
    c.set_read_timeout(Some(std::time::Duration::from_secs(5)))
        .unwrap();
    c.write_all(&frame(8, 0, 1, &[0, 1, 0, 0])).unwrap();
    frames_until(&mut c, &mut buf, |kind, flags, id, p| {
        if kind == 0 && id == 1 {
            body.extend_from_slice(p);
        }
        id == 1 && flags & 1 != 0
    });
    assert!(String::from_utf8_lossy(&body).contains("hello from init"));
}

#[test]
fn curl_http2_prior_knowledge() {
    let Ok(v) = std::process::Command::new("curl").arg("-V").output() else {
        return;
    };
    if !String::from_utf8_lossy(&v.stdout).contains("HTTP2") {
        return;
    }
    let server = start(&[]);
    let url = format!("http://127.0.0.1:{}/", server.port);
    let curl = |args: &[&str]| {
        let out = std::process::Command::new("curl")
            .args(["-s", "--http2-prior-knowledge"])
            .args(args)
            .output()
            .unwrap();
        String::from_utf8_lossy(&out.stdout).into_owned()
    };
    let text = curl(&["-w", "\n%{http_version} %{http_code}", &url]);
    assert!(text.ends_with("\n2 200"), "{text}");
    assert!(text.contains("hello from init"));
    // Several streams on one connection.
    let text = curl(&["-o", "/dev/null", "-w", "%{http_code}\n", &url, &url, &url]);
    assert_eq!(text.matches("200\n").count(), 3, "{text}");
}
