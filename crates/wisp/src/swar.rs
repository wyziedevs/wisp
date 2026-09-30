//! Eight bytes at a time in a `u64` (SWAR): the scans that find the few
//! bytes of a text that need work (escaping, a line break in a header), at
//! a handful of instructions per eight bytes.
//!
//! Each test sets the high bit of the bytes it matches. Carries and
//! borrows only move toward higher bytes, so the lowest bit set is always a
//! true match (`first` finds it); bits above it may not be.

const LO: u64 = 0x0101_0101_0101_0101;
const HI: u64 = 0x8080_8080_8080_8080;

/// The eight bytes at `b[i..]`, the first in the low byte.
#[inline(always)]
pub fn word(b: &[u8], i: usize) -> u64 {
    let mut w = [0; 8];
    w.copy_from_slice(&b[i..i + 8]);
    u64::from_le_bytes(w)
}

/// The fewer than eight bytes of `rest`, and `fill` after them: read in two
/// overlapping halves (or three bytes), as a copy of a length not known in
/// advance would be a call to `memcpy`.
#[inline(always)]
pub fn tail(rest: &[u8], fill: u8) -> u64 {
    let n = rest.len();
    debug_assert!(n < 8);
    let x = if n >= 4 {
        let half = |i: usize| {
            u64::from(u32::from_le_bytes([
                rest[i],
                rest[i + 1],
                rest[i + 2],
                rest[i + 3],
            ]))
        };
        half(0) | half(n - 4) << (8 * (n - 4))
    } else if n > 0 {
        let byte = |i: usize| u64::from(rest[i]) << (8 * i);
        byte(0) | byte(n / 2) | byte(n - 1)
    } else {
        0
    };
    x | (LO * u64::from(fill)) << (8 * n)
}

/// Bytes that are `v`.
#[inline(always)]
pub fn eq(x: u64, v: u8) -> u64 {
    let y = x ^ (LO * v as u64);
    y.wrapping_sub(LO) & !y & HI
}

/// Bytes below `n`, which is at most 128.
#[inline(always)]
pub fn below(x: u64, n: u8) -> u64 {
    x.wrapping_sub(LO * n as u64) & !x & HI
}

/// Bytes above `n`, which is at most 127.
#[inline(always)]
pub fn above(x: u64, n: u8) -> u64 {
    (x.wrapping_add(LO * (127 - n) as u64) | x) & HI
}

/// Whether `test` matches no byte of `b`. Past eight bytes, the last word
/// overlaps the one before; below, the bytes are padded with `a`, which
/// `test` must not match.
#[inline(always)]
pub fn none(b: &[u8], test: impl Fn(u64) -> u64) -> bool {
    if b.len() < 8 {
        return test(tail(b, b'a')) == 0;
    }
    let mut hit = test(word(b, b.len() - 8));
    for w in b.as_chunks::<8>().0 {
        hit |= test(u64::from_le_bytes(*w));
    }
    hit == 0
}

/// Where the first match of a nonzero `mask` is, from 0 to 7.
#[inline(always)]
pub fn first(mask: u64) -> usize {
    mask.trailing_zeros() as usize / 8
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every byte value, alone and at every place in a word among others,
    /// against the obvious byte test.
    #[test]
    fn tests_match_byte_by_byte() {
        type Test = (fn(u64) -> u64, fn(u8) -> bool);
        let tests: [Test; 6] = [
            (|x| eq(x, 0), |b| b == 0),
            (|x| eq(x, b'"'), |b| b == b'"'),
            (|x| eq(x, 0xff), |b| b == 0xff),
            (|x| below(x, 0x20), |b| b < 0x20),
            (|x| below(x, 128), |b| b < 128),
            (|x| above(x, 0x7e), |b| b > 0x7e),
        ];
        for (test, want) in tests {
            for v in 0..=255u8 {
                for at in 0..8 {
                    for other in [b'a', 0, 0x7f, 0x80, 0xff, v] {
                        let mut w = [other; 8];
                        w[at] = v;
                        let mask = test(u64::from_le_bytes(w));
                        let first_want = w.iter().position(|&b| want(b));
                        assert_eq!(mask != 0, first_want.is_some(), "{w:?}");
                        if let Some(p) = first_want {
                            assert_eq!(first(mask), p, "{w:?}");
                        }
                    }
                }
            }
        }
    }

    /// Random words, where bytes past a match may be flagged too.
    #[test]
    fn the_first_match_is_exact() {
        let mut rng = crate::fuzz::Rng::new(9);
        for _ in 0..200_000 {
            let w = rng
                .next()
                .to_le_bytes()
                .map(|b| if rng.one_in(3) { b'"' } else { b });
            let mask = eq(u64::from_le_bytes(w), b'"') | below(u64::from_le_bytes(w), 9);
            let want = w.iter().position(|&b| b == b'"' || b < 9);
            assert_eq!(want, (mask != 0).then(|| first(mask)), "{w:?}");
        }
    }

    /// Every byte value at every place of names and values 1 to 17 bytes
    /// long, against the rule byte by byte.
    #[test]
    fn header_checks_match_byte_by_byte() {
        use crate::cx::valid_header;
        let name_ok = |b: u8| b.is_ascii_graphic() && b != b':';
        let value_ok = |b: u8| b != b'\r' && b != b'\n' && b != 0;
        assert!(!valid_header("", "x") && valid_header("x", ""));
        for len in 1..=17 {
            for at in 0..len {
                for v in 0..=255u8 {
                    let mut b = vec![b'a'; len];
                    b[at] = v;
                    // Names and values are `&str`: the other bytes of a
                    // character are what the rule sees for them.
                    let Ok(s) = std::str::from_utf8(&b) else {
                        continue;
                    };
                    assert_eq!(valid_header(s, "v"), name_ok(v), "{b:?}");
                    assert_eq!(valid_header("n", s), value_ok(v), "{b:?}");
                }
            }
        }
        for s in ["é", "ünï", "\u{2028}"] {
            assert!(!valid_header(s, "v") && valid_header("n", s));
        }
    }

    #[test]
    fn words_and_tails() {
        let b: Vec<u8> = (1..=17).collect();
        assert_eq!(word(&b, 0), u64::from_le_bytes([1, 2, 3, 4, 5, 6, 7, 8]));
        assert_eq!(
            word(&b, 9),
            u64::from_le_bytes([10, 11, 12, 13, 14, 15, 16, 17])
        );
        for n in 0..8 {
            let mut want = [b'z'; 8];
            want[..n].copy_from_slice(&b[..n]);
            assert_eq!(tail(&b[..n], b'z'), u64::from_le_bytes(want), "{n}");
        }
    }
}
