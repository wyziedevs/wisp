//! WebSocket frames (RFC 6455) as a test client writes and reads them,
//! shared by the tests of `wisp` and of the test app.

use std::io::Read;

/// A client frame: `first` is its first byte (FIN and opcode). Always
/// masked, as the RFC requires.
pub fn frame(first: u8, payload: &[u8]) -> Vec<u8> {
    let mask = [0x37, 0xfa, 0x21, 0x3d];
    let mut f = vec![first];
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
    f.extend(payload.iter().enumerate().map(|(i, b)| b ^ mask[i % 4]));
    f
}

/// The server's next frame: first byte and payload (never masked).
pub fn read(from: &mut impl Read) -> (u8, Vec<u8>) {
    let mut head = [0u8; 2];
    from.read_exact(&mut head).unwrap();
    assert_eq!(head[1] & 0x80, 0, "a server does not mask");
    let len = match head[1] & 0x7f {
        126 => {
            let mut n = [0u8; 2];
            from.read_exact(&mut n).unwrap();
            u16::from_be_bytes(n) as usize
        }
        127 => {
            let mut n = [0u8; 8];
            from.read_exact(&mut n).unwrap();
            u64::from_be_bytes(n) as usize
        }
        n => n as usize,
    };
    let mut payload = vec![0; len];
    from.read_exact(&mut payload).unwrap();
    (head[0], payload)
}
