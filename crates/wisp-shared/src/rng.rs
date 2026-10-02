//! The seeded generator of every crate's property tests, so a failure
//! repeats: xorshift64*, small, fast and good enough to find edge cases.
//! Not for anything secret.

pub struct Rng(pub u64);

impl Rng {
    /// From any seed, 0 too.
    pub fn new(seed: u64) -> Rng {
        Rng(seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1)
    }

    pub fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }

    /// From 0 to `n - 1`; 0 when `n` is 0.
    pub fn below(&mut self, n: usize) -> usize {
        if n == 0 {
            0
        } else {
            (self.next() % n as u64) as usize
        }
    }

    pub fn one_in(&mut self, n: usize) -> bool {
        self.below(n) == 0
    }

    pub fn pick<T: Copy>(&mut self, from: &[T]) -> T {
        from[self.below(from.len())]
    }

    /// `n` bytes from `alphabet`, or any bytes if it is empty.
    pub fn bytes(&mut self, n: usize, alphabet: &[u8]) -> Vec<u8> {
        (0..n)
            .map(|_| {
                if alphabet.is_empty() {
                    self.next() as u8
                } else {
                    self.pick(alphabet)
                }
            })
            .collect()
    }

    /// Up to `max` bytes from `alphabet`.
    pub fn upto(&mut self, max: usize, alphabet: &[u8]) -> Vec<u8> {
        let n = self.below(max + 1);
        self.bytes(n, alphabet)
    }

    /// Text of up to `max` characters, ASCII mostly, with some that need
    /// escaping or take several bytes.
    pub fn text(&mut self, max: usize) -> String {
        const SOME: &[char] = &[
            'a', 'z', '0', ' ', '"', '\\', '/', '%', '+', '&', '=', ';', ',', '|', '<', '\n', '\r',
            '\t', '\0', '\u{7f}', 'é', 'ü', '€', '😀', '\u{2028}', '\u{fffd}',
        ];
        let n = self.below(max + 1);
        (0..n)
            .map(|_| {
                if self.one_in(3) {
                    self.pick(SOME)
                } else {
                    (b'a' + self.below(26) as u8) as char
                }
            })
            .collect()
    }

    /// `n` pieces picked from `from`, joined.
    pub fn pieces(&mut self, from: &[&str], n: usize) -> String {
        (0..n).map(|_| self.pick(from)).collect()
    }
}
