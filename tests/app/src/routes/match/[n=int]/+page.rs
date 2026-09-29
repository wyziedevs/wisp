struct Data {
    n: u64,
}

/// The matcher lets only digits through, so `n` is always a `u64`.
fn load(n: u64) -> Data {
    Data { n }
}
