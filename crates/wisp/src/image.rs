//! `Image`: a picture a visitor sent, as an action takes it:
//!
//! ```ignore
//! #[action]
//! fn avatar(#[validate(max_size = 1 * MB)] avatar: Image) {
//!     USERS.update(cx.signed_in()?, |u| u.avatar = Some(avatar));
//! }
//! ```
//!
//! Its type is what its bytes say, not what the browser claimed: PNG, JPEG,
//! GIF, WebP or AVIF, by their first bytes. Anything else, SVG among them
//! (it can carry script), is a 422 by the field. At most 2 MB
//! ([`MAX_SIZE`](crate::MAX_SIZE)) unless `max_size` says otherwise; the
//! route's body limit has room enough for it, made at build time. The
//! form needs no `enctype`, nor its file input `accept="image/*"`: the
//! build adds them.
//!
//! It is kept in a table as a `data:` URL, which is also its JSON, and a
//! `+server.rs` handler returns it as it is (`-> Option<Image>`): the bytes,
//! their type, an ETag (a 304 when the browser has them) and `no-cache`, so
//! a changed picture shows at once.

use crate::json::Problems;
use crate::sign::{base64, unbase64};
use crate::{FromJson, Json, Response, Value};
use std::fmt;
use std::sync::Arc;

/// What is wrong with a file sent as an image that is not one.
pub(crate) const NOT_AN_IMAGE: &str = "must be a PNG, JPEG, GIF, WebP or AVIF image";

/// A PNG, JPEG, GIF, WebP or AVIF image. Cloning it shares the bytes.
#[derive(Clone, PartialEq, Eq)]
pub struct Image {
    kind: &'static str,
    bytes: Arc<[u8]>,
}

impl Image {
    /// `bytes` as an image, if they are one of the kinds it takes.
    pub fn new(bytes: impl Into<Arc<[u8]>>) -> Option<Image> {
        let bytes = bytes.into();
        let kind = sniff(&bytes)?;
        Some(Image { kind, bytes })
    }

    /// Its media type: `image/png`, `image/jpeg`, `image/gif`, `image/webp`
    /// or `image/avif`.
    pub fn kind(&self) -> &'static str {
        self.kind
    }

    /// The image's bytes as uploaded.
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// Its size in bytes.
    pub fn len(&self) -> usize {
        self.bytes.len()
    }

    /// No bytes: the visitor left the file input empty.
    pub fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }

    /// `data:image/png;base64,…`, for `<img src={user.avatar.data_url()}>`
    /// of a small one.
    pub fn data_url(&self) -> String {
        let mut out = String::with_capacity(self.bytes.len() * 4 / 3 + 32);
        self.write_url(&mut out);
        out
    }

    fn write_url(&self, out: &mut String) {
        out.push_str("data:");
        out.push_str(self.kind);
        out.push_str(";base64,");
        base64(out, &self.bytes, false);
    }
}

/// The kind of image `b` is, by its first bytes.
pub(crate) fn sniff(b: &[u8]) -> Option<&'static str> {
    if b.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Some("image/png");
    }
    if b.starts_with(b"\xff\xd8\xff") {
        return Some("image/jpeg");
    }
    if b.starts_with(b"GIF87a") || b.starts_with(b"GIF89a") {
        return Some("image/gif");
    }
    if b.len() >= 12 && b.starts_with(b"RIFF") && &b[8..12] == b"WEBP" {
        return Some("image/webp");
    }
    // An ISO media file whose `ftyp` box names an AVIF brand, first or
    // among the compatible ones (after the minor version).
    if b.len() >= 16 && &b[4..8] == b"ftyp" {
        let size = u32::from_be_bytes([b[0], b[1], b[2], b[3]]) as usize;
        let ftyp = &b[..size.clamp(16, b.len())];
        let mut brands =
            (ftyp[8..12].as_chunks::<4>().0.iter()).chain(ftyp[16..].as_chunks::<4>().0);
        if brands.any(|x| x == b"avif" || x == b"avis") {
            return Some("image/avif");
        }
    }
    None
}

/// `n` bytes as people say it: `1 MB`, `64 KB`, `1500 bytes`.
pub(crate) fn size_text(n: usize) -> String {
    use crate::{KB, MB};
    match n {
        0 => "0 bytes".into(),
        n if n % MB == 0 => format!("{} MB", n / MB),
        n if n % KB == 0 => format!("{} KB", n / KB),
        n => format!("{n} bytes"),
    }
}

impl fmt::Debug for Image {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Image({}, {} bytes)", self.kind, self.bytes.len())
    }
}

impl Json for Image {
    fn json(&self, out: &mut String) {
        out.push('"');
        self.write_url(out);
        out.push('"');
    }
}

impl FromJson for Image {
    fn from_json(v: &Value, p: &mut Problems) -> Option<Image> {
        let url = String::from_json(v, p)?;
        let data = url
            .strip_prefix("data:")
            .and_then(|u| u.split_once(";base64,"))
            .map(|(_, data)| data);
        let mut bytes = vec![0; data.map_or(0, |d| d.len() / 4 * 3 + 3)];
        let image = data.and_then(|d| unbase64(d, &mut bytes)).and_then(|n| {
            bytes.truncate(n);
            Image::new(bytes)
        });
        if image.is_none() {
            p.add(format!("{NOT_AN_IMAGE}, as a data: URL"));
        }
        image
    }
}

/// The image as a response: its bytes and type, an ETag, and `no-cache`
/// (the browser keeps it, and asks whether it changed before it shows it).
/// `nosniff` keeps browsers from reading it as anything else.
impl From<Image> for Response {
    fn from(image: Image) -> Response {
        let mut r = Response::new(image.kind, image.bytes.to_vec());
        r.headers
            .push(("etag".into(), crate::rest::etag(&image.bytes)));
        r.headers.push(("cache-control".into(), "no-cache".into()));
        r.headers
            .push(("x-content-type-options".into(), "nosniff".into()));
        r
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PNG: &[u8] = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR";

    #[test]
    fn kinds_by_their_bytes() {
        let avif = b"\0\0\0\x1cftypmif1\0\0\0\0mif1avifmiaf";
        let avif_first = b"\0\0\0\x14ftypavif\0\0\0\0mif1";
        for (bytes, kind) in [
            (PNG, Some("image/png")),
            (b"\xff\xd8\xff\xe0\0\x10JFIF", Some("image/jpeg")),
            (b"GIF89a\x01\0", Some("image/gif")),
            (b"GIF87a", Some("image/gif")),
            (b"RIFF\x24\0\0\0WEBPVP8 ", Some("image/webp")),
            (avif, Some("image/avif")),
            (avif_first, Some("image/avif")),
            (b"\0\0\0\x14ftypisom\0\0\0\0isomavif", None), // past its box
            (b"\0\0\0\x14ftypmp42\0\0\0\0mp41", None),
            (
                b"<svg xmlns=\"http://www.w3.org/2000/svg\"><script/></svg>",
                None,
            ),
            (b"RIFF\x24\0\0\0WAVE", None),
            (b"\x89PNG", None),
            (b"", None),
        ] {
            assert_eq!(sniff(bytes), kind, "{bytes:?}");
        }
        assert!(Image::new(&b"GIF"[..]).is_none());
        assert_eq!(size_text(crate::MB), "1 MB");
        assert_eq!(size_text(64 * crate::KB), "64 KB");
        assert_eq!(size_text(1500), "1500 bytes");
    }

    #[test]
    fn kept_as_a_data_url_and_answered_as_itself() {
        let image = Image::new(PNG).unwrap();
        let json = crate::to_json(&image);
        assert_eq!(json, "\"data:image/png;base64,iVBORw0KGgoAAAANSUhEUg==\"");
        assert_eq!(image.data_url(), json.trim_matches('"'));
        let back: Image = crate::from_json(json.as_bytes()).unwrap();
        assert_eq!(back, image);
        for bad in [
            "\"data:image/svg+xml;base64,PHN2Zz4=\"",
            "\"data:image/png,x\"",
            "\"iVBORw0KGgo=\"",
            "\"data:image/png;base64,!!\"",
            "3",
        ] {
            let r: crate::Result<Image> = crate::from_json(bad.as_bytes());
            assert_eq!(r.unwrap_err().status(), 422, "{bad}");
        }
        let r = Response::from(image);
        assert_eq!((r.status, &*r.content_type), (200, "image/png"));
        assert_eq!(r.body, PNG);
        let header = |n: &str| {
            r.headers
                .iter()
                .find(|(k, _)| k == n)
                .map(|(_, v)| v.as_str())
        };
        assert_eq!(header("cache-control"), Some("no-cache"));
        assert_eq!(header("x-content-type-options"), Some("nosniff"));
        assert!(header("etag").is_some_and(|t| t.len() == 18));
    }

    /// Bytes that are not a whole image never panic, whatever their length.
    #[test]
    fn short_and_odd_input_is_refused() {
        let mut ftyp = b"\0\0\0\x10ftypavif\0\0\0\0".to_vec();
        for n in 0..ftyp.len() {
            let _ = sniff(&ftyp[..n]);
        }
        ftyp[3] = 0xff; // a size past the end
        assert_eq!(sniff(&ftyp), Some("image/avif"));
        ftyp[3] = 0; // a size under the header
        let _ = sniff(&ftyp);
        for data in ["", "=", "AA", "A", "AAAAA", "====", "\u{e9}\u{e9}"] {
            let mut p = Problems::default();
            let v = Value::String(format!("data:image/png;base64,{data}"));
            assert!(Image::from_json(&v, &mut p).is_none(), "{data:?}");
        }
    }
}
