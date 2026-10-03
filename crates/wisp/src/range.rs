//! `Range` for files: a video that seeks, a download that resumes. One
//! range of bytes, as `bytes=a-b`, `bytes=a-` or `bytes=-n`; a request for
//! several gets the whole file, which a server may always answer.

use crate::cx::Cx;
use crate::http::{Body, Reply};
use std::borrow::Cow;

/// `reply`, a 200 with a file, as the 206 of the range the request asks
/// for, or a 416 when the file has no such bytes. Whether it was a range
/// (so the file is not gzipped as well).
pub(crate) fn apply(cx: &Cx, reply: &mut Reply, etag: Option<&str>) -> bool {
    reply
        .headers
        .push((Cow::Borrowed("accept-ranges"), Cow::Borrowed("bytes")));
    let Some(want) = cx.header("range") else {
        return false;
    };
    // The range is for the file the client has: `if-range` names it.
    if let Some(have) = cx.header("if-range")
        && etag != Some(have)
    {
        return false;
    }
    let len = reply.bytes().len();
    let (from, to) = match parse(want, len) {
        Some(Some(r)) => r,
        Some(None) => {
            reply.status = 416;
            reply.body = Body::Static(b"");
            reply.headers.push((
                Cow::Borrowed("content-range"),
                Cow::Owned(format!("bytes */{len}")),
            ));
            return true;
        }
        None => return false,
    };
    reply.status = 206;
    reply.headers.push((
        Cow::Borrowed("content-range"),
        Cow::Owned(format!("bytes {from}-{}/{len}", to - 1)),
    ));
    reply.body = match std::mem::replace(&mut reply.body, Body::Static(b"")) {
        Body::Static(b) => Body::Static(&b[from..to]),
        Body::Bytes(mut b) => {
            b.truncate(to);
            b.drain(..from);
            Body::Bytes(b)
        }
        other => other,
    };
    true
}

/// The bytes `from..to` of a file of `len` that a `range` header asks for:
/// `Some(None)` when the file has none of them, `None` when the header is
/// not one range of bytes (it is then ignored).
fn parse(header: &str, len: usize) -> Option<Option<(usize, usize)>> {
    let spec = header.trim().strip_prefix("bytes=")?;
    if spec.contains(',') {
        return None;
    }
    let (a, b) = spec.split_once('-')?;
    let (a, b) = (a.trim(), b.trim());
    let num = |s: &str| {
        (!s.is_empty() && s.bytes().all(|c| c.is_ascii_digit()))
            .then(|| s.parse().unwrap_or(usize::MAX))
    };
    if a.is_empty() {
        // The last `n` bytes.
        let n = num(b)?;
        return Some((n > 0 && len > 0).then(|| (len.saturating_sub(n), len)));
    }
    let from = num(a)?;
    let last = if b.is_empty() { usize::MAX } else { num(b)? };
    // `b` before `a` is not a range at all.
    if last < from {
        return None;
    }
    Some((from < len).then(|| (from, last.saturating_add(1).min(len))))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranges() {
        assert_eq!(parse("bytes=0-3", 10), Some(Some((0, 4))));
        assert_eq!(parse("bytes=5-", 10), Some(Some((5, 10))));
        assert_eq!(parse("bytes=-3", 10), Some(Some((7, 10))));
        assert_eq!(parse("bytes=-30", 10), Some(Some((0, 10))));
        assert_eq!(parse("bytes=8-100", 10), Some(Some((8, 10))));
        assert_eq!(parse("bytes=0-0", 10), Some(Some((0, 1))));
    }

    #[test]
    fn what_the_file_has_none_of() {
        assert_eq!(parse("bytes=10-", 10), Some(None));
        assert_eq!(parse("bytes=50-60", 10), Some(None));
        assert_eq!(parse("bytes=-0", 10), Some(None));
        assert_eq!(parse("bytes=-5", 0), Some(None));
        assert_eq!(parse("bytes=0-", 0), Some(None));
    }

    #[test]
    fn what_is_not_a_range_is_ignored() {
        assert_eq!(parse("bytes=0-1,4-5", 10), None);
        assert_eq!(parse("items=0-1", 10), None);
        assert_eq!(parse("bytes=5-2", 10), None);
        assert_eq!(parse("bytes=x-2", 10), None);
        assert_eq!(parse("bytes=1", 10), None);
        assert_eq!(parse("bytes=-", 10), None);
    }
}
