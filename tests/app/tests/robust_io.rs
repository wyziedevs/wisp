//! The server's I/O on the wire, where it must hold up against clients that
//! misbehave or merely sit there: memory per idle connection, answers that
//! must arrive even when the client keeps sending, HTTP/1.0 rules.

#![cfg(not(target_arch = "wasm32"))]

mod common;

use common::*;
use std::io::{BufReader, Read, Write};
use std::time::Duration;

/// The server's private memory, in KB.
fn memory_kb(pid: u32) -> u64 {
    #[cfg(target_os = "linux")]
    {
        let status = std::fs::read_to_string(format!("/proc/{pid}/status")).unwrap();
        let line = status.lines().find(|l| l.starts_with("VmRSS:")).unwrap();
        line.split_whitespace().nth(1).unwrap().parse().unwrap()
    }
    #[cfg(windows)]
    {
        let out = std::process::Command::new("powershell")
            .args([
                "-NoProfile",
                "-Command",
                &format!("(Get-Process -Id {pid}).PrivateMemorySize64"),
            ])
            .output()
            .unwrap();
        String::from_utf8_lossy(&out.stdout)
            .trim()
            .parse::<u64>()
            .unwrap()
            / 1024
    }
    #[cfg(not(any(target_os = "linux", windows)))]
    {
        let _ = pid;
        0
    }
}

#[test]
fn idle_connections_hold_no_buffers() {
    // Each answered once, then left open: a connection waiting for its next
    // request holds none of the 24 KB of buffers a request works in. (Once
    // each held them in a read that had nothing to read: tokio on Windows
    // says every new socket is readable, and the epoll tries every new one.)
    let server = start(&[("WISP_IO", "epoll")]);
    let mut warm = BufReader::new(connect(server.port));
    let get = format!("GET / HTTP/1.1\r\nhost: 127.0.0.1:{}\r\n\r\n", server.port);
    warm.get_mut().write_all(get.as_bytes()).unwrap();
    read_answer(&mut warm);
    std::thread::sleep(Duration::from_millis(300));
    let before = memory_kb(server.child.id());
    let n = 800;
    let mut conns: Vec<_> = (0..n)
        .map(|_| BufReader::new(connect(server.port)))
        .collect();
    for c in &mut conns {
        c.get_mut().write_all(get.as_bytes()).unwrap();
    }
    for c in &mut conns {
        read_answer(c);
    }
    std::thread::sleep(Duration::from_millis(500));
    let per = (memory_kb(server.child.id()).saturating_sub(before)) * 1024 / n;
    assert!(per < 12 * 1024, "{per} bytes a connection");
}

#[test]
fn a_refusal_arrives_while_the_client_still_sends() {
    // Told 413 from the head alone while the body is still on its way: the
    // answer is not lost to a reset when the server closes.
    let server = start(&[]);
    for _ in 0..5 {
        let mut c = connect(server.port);
        let head = format!(
            "POST /echo HTTP/1.1\r\nhost: 127.0.0.1:{}\r\ncontent-length: 10000000\r\n\r\n",
            server.port
        );
        c.write_all(head.as_bytes()).unwrap();
        let mut w = c.try_clone().unwrap();
        // A server that stops reading leaves the writer stuck: it gives up.
        w.set_write_timeout(Some(Duration::from_secs(5))).unwrap();
        c.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
        let writer = std::thread::spawn(move || {
            let chunk = [b'x'; 16 * 1024];
            for _ in 0..64 {
                if w.write_all(&chunk).is_err() {
                    return;
                }
            }
        });
        std::thread::sleep(Duration::from_millis(100));
        let mut got = Vec::new();
        let _ = c.read_to_end(&mut got);
        writer.join().unwrap();
        let got = String::from_utf8_lossy(&got);
        assert_eq!(status(&got), 413, "{got:?}");
    }
}

#[test]
fn an_http_1_0_client_gets_no_100_continue() {
    let server = start(&[]);
    let mut c = connect(server.port);
    c.write_all(b"POST /echo HTTP/1.0\r\nexpect: 100-continue\r\ncontent-length: 2\r\n\r\n")
        .unwrap();
    c.set_read_timeout(Some(Duration::from_millis(300)))
        .unwrap();
    let mut first = [0; 64];
    let early = c.read(&mut first).unwrap_or(0);
    assert_eq!(early, 0, "{:?}", String::from_utf8_lossy(&first[..early]));
    c.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    c.write_all(b"{}").unwrap();
    let mut got = String::new();
    let _ = c.read_to_string(&mut got);
    assert!(
        got.starts_with("HTTP/1.1 ") && !got.contains("100 Continue"),
        "{got}"
    );
}

#[test]
fn http_1_1_must_name_one_host() {
    let server = start(&[]);
    let sent = |raw: &[u8]| status(&server.send(raw));
    assert_eq!(sent(b"GET / HTTP/1.1\r\nconnection: close\r\n\r\n"), 400);
    let two = b"GET / HTTP/1.1\r\nhost: a\r\nhost: b\r\nconnection: close\r\n\r\n";
    assert_eq!(sent(two), 400);
    assert_eq!(
        sent(b"GET / HTTP/1.1\r\nhost: a\r\nconnection: close\r\n\r\n"),
        200
    );
    assert_eq!(sent(b"GET / HTTP/1.0\r\n\r\n"), 200);
}

#[test]
fn a_client_that_leaves_a_quiet_stream_after_sending_much_frees_its_connection() {
    // Behind a stream that sends nothing more, a client sends more than a
    // connection keeps, then goes: its leaving is still heard, and its
    // connection freed, though the stream never writes again.
    let s = start(&[("WISP_MAX_CONNS", "1")]);
    let mut c = BufReader::new(connect(s.port));
    c.get_mut()
        .write_all(b"GET /t/quiet HTTP/1.1\r\nhost: x\r\n\r\n")
        .unwrap();
    let head = read_head(&mut c);
    assert!(head.starts_with("HTTP/1.1 200"), "{head}");
    let held = s.send(b"GET / HTTP/1.1\r\nhost: x\r\nconnection: close\r\n\r\n");
    assert_ne!(status(&held), 200, "the cap holds while it is open: {held}");
    let _ = c.get_mut().write_all(&vec![b'x'; 200_000]);
    std::thread::sleep(Duration::from_millis(100));
    drop(c);
    let until = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        let answer = s.send(b"GET / HTTP/1.1\r\nhost: x\r\nconnection: close\r\n\r\n");
        if status(&answer) == 200 {
            break;
        }
        assert!(std::time::Instant::now() < until, "still held: {answer}");
        std::thread::sleep(Duration::from_millis(50));
    }
}
