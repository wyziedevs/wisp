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
