//! The clock and the Date header.

use super::*;

// The time, kept by a background thread that ticks once a second, so the
// request path reads an atomic instead of the OS clock (two clock reads were
// ~2% of a plaintext request). Whole seconds are all it needs: the Date
// header has no finer resolution, and timeouts are swept once a second. The
// same tradeoff nginx makes with its cached time.

/// The clock in one word, so a reader never sees the two halves out of
/// step: the unix second (for the Date header) in the high 32 bits, seconds
/// since the clock started (monotonic, what request deadlines are counted
/// in) in the low 32. 0 until the clock starts.
pub(super) static TICK: AtomicU64 = AtomicU64::new(0);

/// One `TICK` word from its unix second and elapsed seconds.
#[inline(always)]
pub(super) fn tick(unix: u64, elapsed: u64) -> u64 {
    (unix << 32) | (elapsed & 0xffff_ffff)
}

/// The unix second of a `TICK` word.
#[inline(always)]
pub(super) fn tick_unix(t: u64) -> u64 {
    t >> 32
}

/// The elapsed seconds of a `TICK` word.
#[inline(always)]
pub(super) fn tick_elapsed(t: u64) -> u64 {
    t & 0xffff_ffff
}

pub(crate) fn seconds() -> u64 {
    tick_elapsed(TICK.load(Ordering::Relaxed))
}

/// The unix second: the clock's, when the server keeps one, else the
/// system's (a host other than the built-in server).
pub(crate) fn now() -> u64 {
    match tick_unix(TICK.load(Ordering::Relaxed)) {
        0 => crate::unix_now(),
        n => n,
    }
}

/// When `seconds()` was 0.
pub(super) static START: OnceLock<Instant> = OnceLock::new();

/// The instant of `deadline`, in `seconds()`, for a tokio timer.
#[cfg(not(target_arch = "wasm32"))]
pub(super) fn instant(deadline: u64) -> tokio::time::Instant {
    let start = *START.get_or_init(Instant::now);
    tokio::time::Instant::from_std(start + Duration::from_secs(deadline))
}

pub(crate) fn start_clock() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        TICK.store(tick(crate::unix_now(), 0), Ordering::Relaxed);
        let start = *START.get_or_init(Instant::now);
        std::thread::Builder::new()
            .name("wisp-clock".into())
            .spawn(move || {
                loop {
                    std::thread::sleep(Duration::from_secs(1));
                    let t = tick(crate::unix_now(), start.elapsed().as_secs());
                    TICK.store(t, Ordering::Relaxed);
                }
            })
            .expect("failed to start clock thread");
    });
}

/// `date: Sun, 06 Nov 1994 08:49:37 GMT\r\n`
pub(super) const DATE_LINE: usize = 37;

thread_local! {
    /// (`TICK` word, its `date` line), formatted at most once a second.
    static DATE: Cell<(u64, [u8; DATE_LINE])> = const { Cell::new((u64::MAX, [0; DATE_LINE])) };
}

/// `content-length`, when the response has a `length`, and `date`, in one
/// piece: the digits are written right to left, ending where the date
/// line, kept formatted, begins.
#[inline(always)]
pub(super) fn length_and_date(w: &mut Vec<u8>, length: Option<usize>) {
    const PREFIX: &[u8] = b"content-length: ";
    const LENGTH_LINE: usize = PREFIX.len() + 20 + 2;
    let mut head = [0u8; LENGTH_LINE + DATE_LINE];
    head[LENGTH_LINE..].copy_from_slice(&date_line());
    let mut start = LENGTH_LINE;
    if let Some(n) = length {
        head[LENGTH_LINE - 2..LENGTH_LINE].copy_from_slice(b"\r\n");
        start = crate::digits(&mut head, LENGTH_LINE - 2, n as u64) - PREFIX.len();
        head[start..start + PREFIX.len()].copy_from_slice(PREFIX);
    }
    w.extend_from_slice(&head[start..]);
}

#[inline(always)]
pub(super) fn date_line() -> [u8; DATE_LINE] {
    let tick = TICK.load(Ordering::Relaxed);
    DATE.with(|c| {
        let (kept, line) = c.get();
        if kept == tick {
            return line;
        }
        let mut line = [0; DATE_LINE];
        line[..6].copy_from_slice(b"date: ");
        line[6..35].copy_from_slice(&http_date(tick_unix(tick)));
        line[35..].copy_from_slice(b"\r\n");
        c.set((tick, line));
        line
    })
}

/// IMF-fixdate, e.g. `Sun, 06 Nov 1994 08:49:37 GMT`.
pub(super) fn http_date(secs: u64) -> [u8; 29] {
    const DAYS: [&[u8; 3]; 7] = [b"Thu", b"Fri", b"Sat", b"Sun", b"Mon", b"Tue", b"Wed"];
    const MONTHS: [&[u8; 3]; 12] = [
        b"Jan", b"Feb", b"Mar", b"Apr", b"May", b"Jun", b"Jul", b"Aug", b"Sep", b"Oct", b"Nov",
        b"Dec",
    ];
    let (days, rem) = (secs / 86_400, secs % 86_400);
    let (year, month, day) = crate::civil(days);

    let two = |n: u64| [b'0' + (n / 10) as u8, b'0' + (n % 10) as u8];
    let mut out = [0u8; 29];
    out[..3].copy_from_slice(DAYS[(days % 7) as usize]);
    out[3..5].copy_from_slice(b", ");
    out[5..7].copy_from_slice(&two(day));
    out[7] = b' ';
    out[8..11].copy_from_slice(MONTHS[month as usize - 1]);
    out[11] = b' ';
    out[12..14].copy_from_slice(&two(year / 100 % 100));
    out[14..16].copy_from_slice(&two(year % 100));
    out[16] = b' ';
    out[17..19].copy_from_slice(&two(rem / 3600));
    out[19] = b':';
    out[20..22].copy_from_slice(&two(rem / 60 % 60));
    out[22] = b':';
    out[23..25].copy_from_slice(&two(rem % 60));
    out[25..29].copy_from_slice(b" GMT");
    out
}
