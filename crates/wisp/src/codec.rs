//! Small codecs the request and response code share: hex digits, request
//! ids and the header check. (Hex and base64 themselves are `wisp_shared`'s,
//! used through `sign`.)

/// A fresh request id: 8 random hex digits per process, then a counter, so
/// two are never alike within a process and, but for a 1 in 4 billion
/// chance, across processes either.
pub(crate) fn new_id() -> String {
    static SEED: std::sync::OnceLock<u32> = std::sync::OnceLock::new();
    static COUNT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let seed = *SEED.get_or_init(|| u32::from_le_bytes(crate::sign::random()));
    let n = COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    crate::hex(&(u64::from(seed) << 32 | u64::from(n)).to_be_bytes())
}

/// A name of visible ASCII but `:`, and a value with no CR, LF or NUL,
/// eight bytes at a time: nothing that would end the line or the field.
pub(crate) fn valid_header(name: &str, value: &str) -> bool {
    use crate::swar::{above, below, eq, none};
    !name.is_empty()
        && none(name.as_bytes(), |x| {
            below(x, 0x21) | above(x, 0x7e) | eq(x, b':')
        })
        && none(value.as_bytes(), |x| eq(x, b'\r') | eq(x, b'\n') | eq(x, 0))
}

/// The value of a hex digit, either case.
pub(crate) fn hex_digit(b: u8) -> Option<u8> {
    (b as char).to_digit(16).map(|d| d as u8)
}
