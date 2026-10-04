//! The `img` feature: `/_img?src=/photo.jpg&w=640&q=75`, a picture of
//! `static/` resized on demand, as next/image does. A route serves it:
//!
//! ```ignore
//! // src/routes/_img/+server.rs
//! fn get(cx: &mut Cx) -> Response {
//!     wisp::img::serve::<crate::App>(cx)
//! }
//! ```
//!
//! Only local files of `static/` (a JPEG, PNG or WebP; in a release
//! binary, the files it embeds), never one a `..` or a drive reaches. `w`
//! is one of [`WIDTHS`] (never more than the picture's own), `q` is 1 to
//! 100 (default 75, for JPEG). A JPEG comes out a JPEG, the rest PNG.
//! Files over 10 MB and pictures over 40 megapixels are a 400, and nothing
//! panics: a bad file is a 400 as well. Results are kept in memory (up to
//! 64 MB, then the cache starts over). With the feature off none of this is
//! compiled in.

use crate::{App, Cx, Response};
use image::{DynamicImage, ImageFormat, ImageReader, imageops::FilterType};
use std::collections::HashMap;
use std::io::Cursor;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Arc, Mutex};

/// The widths a request may ask for (next/image's).
pub const WIDTHS: [u32; 13] = [
    64, 96, 128, 256, 384, 640, 750, 828, 1080, 1200, 1920, 2048, 3840,
];

const MAX_FILE: usize = 10 << 20;
const MAX_PIXELS: u64 = 40_000_000;
const MAX_CACHE: usize = 64 << 20;

type Key = (String, u32, u8, usize);
type Kept = Arc<(Vec<u8>, &'static str)>;

struct Cache {
    map: HashMap<Key, Kept>,
    bytes: usize,
}

static CACHE: Mutex<Option<Cache>> = Mutex::new(None);

fn fail(status: u16, why: &'static str) -> Response {
    Response::text(why).with_status(status)
}

/// Answers a request for `/_img` (see the top).
pub fn serve<A: App>(cx: &Cx) -> Response {
    let (Some(src), Some(w)) = (cx.query("src"), cx.query("w")) else {
        return fail(400, "src and w are needed");
    };
    let Some(w) = w.parse::<u32>().ok().filter(|w| WIDTHS.contains(w)) else {
        return fail(400, "w is not an allowed width");
    };
    let q = match cx.query("q") {
        None => 75,
        Some(q) => match q.parse::<u8>() {
            Ok(q @ 1..=100) => q,
            _ => return fail(400, "q is 1 to 100"),
        },
    };
    let Some(rel) = crate::http::safe_relative_path(&src) else {
        return fail(400, "src is a path of static/ starting with /");
    };
    let Some(bytes) = load::<A>(&src, &rel) else {
        return fail(404, "no such image");
    };
    if bytes.len() > MAX_FILE {
        return fail(400, "the image is too large");
    }
    let key = (rel, w, q, bytes.len());
    if let Some(hit) = cache_get(&key) {
        return reply(&hit);
    }
    // A decoder is code a hostile file runs: a panic is a 400, never a crash.
    match catch_unwind(AssertUnwindSafe(|| resize(&bytes, w, q))) {
        Ok(Some(done)) => {
            let done = Arc::new(done);
            cache_put(key, done.clone());
            reply(&done)
        }
        _ => fail(400, "not an image Wisp can resize"),
    }
}

fn reply(k: &Kept) -> Response {
    Response::new(k.1, k.0.clone())
        .with_header("cache-control", "public, max-age=86400")
        .with_header("x-content-type-options", "nosniff")
}

/// The file, from the binary's embedded `static/` or, in dev, from disk.
fn load<A: App>(src: &str, rel: &str) -> Option<Vec<u8>> {
    let ext = rel.rsplit('.').next()?.to_ascii_lowercase();
    if !matches!(ext.as_str(), "jpg" | "jpeg" | "png" | "webp") {
        return None;
    }
    if let Some(a) = A::asset(src) {
        return Some(a.body.to_vec());
    }
    let path = std::path::Path::new("static").join(rel);
    let meta = std::fs::metadata(&path).ok().filter(|m| m.is_file())?;
    (meta.len() <= (MAX_FILE as u64)).then(|| std::fs::read(path).ok())?
}

fn resize(bytes: &[u8], w: u32, q: u8) -> Option<(Vec<u8>, &'static str)> {
    let reader = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .ok()?;
    let format = reader.format()?;
    if !matches!(
        format,
        ImageFormat::Jpeg | ImageFormat::Png | ImageFormat::WebP
    ) {
        return None;
    }
    let (iw, ih) = reader.into_dimensions().ok()?;
    if u64::from(iw) * u64::from(ih) > MAX_PIXELS || iw == 0 || ih == 0 {
        return None;
    }
    let mut img = image::load_from_memory_with_format(bytes, format).ok()?;
    if w < iw {
        let h = (u64::from(ih) * u64::from(w) / u64::from(iw)).max(1) as u32;
        img = img.resize_exact(w, h, FilterType::CatmullRom);
    }
    let mut out = Vec::new();
    if format == ImageFormat::Jpeg {
        let enc = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, q);
        DynamicImage::ImageRgb8(img.to_rgb8())
            .write_with_encoder(enc)
            .ok()?;
        Some((out, "image/jpeg"))
    } else {
        img.write_to(&mut Cursor::new(&mut out), ImageFormat::Png)
            .ok()?;
        Some((out, "image/png"))
    }
}

fn cache_get(key: &Key) -> Option<Kept> {
    let g = CACHE.lock().ok()?;
    g.as_ref()?.map.get(key).cloned()
}

fn cache_put(key: Key, v: Kept) {
    let Ok(mut g) = CACHE.lock() else {
        return;
    };
    let c = g.get_or_insert_with(|| Cache {
        map: HashMap::new(),
        bytes: 0,
    });
    if c.bytes + v.0.len() > MAX_CACHE {
        c.map.clear();
        c.bytes = 0;
    }
    c.bytes += v.0.len();
    c.map.insert(key, v);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png(w: u32, h: u32) -> Vec<u8> {
        let mut out = Vec::new();
        DynamicImage::new_rgba8(w, h)
            .write_to(&mut Cursor::new(&mut out), ImageFormat::Png)
            .unwrap();
        out
    }

    #[test]
    fn resizes_and_never_grows() {
        let (body, mime) = resize(&png(1000, 500), 640, 75).unwrap();
        assert_eq!(mime, "image/png");
        let img = image::load_from_memory(&body).unwrap();
        assert_eq!((img.width(), img.height()), (640, 320));
        let (body, _) = resize(&png(100, 50), 640, 75).unwrap();
        assert_eq!(image::load_from_memory(&body).unwrap().width(), 100);
    }

    #[test]
    fn jpeg_stays_jpeg_and_junk_is_refused() {
        let mut jpg = Vec::new();
        DynamicImage::new_rgb8(40, 40)
            .write_to(&mut Cursor::new(&mut jpg), ImageFormat::Jpeg)
            .unwrap();
        assert_eq!(resize(&jpg, 64, 50).unwrap().1, "image/jpeg");
        assert!(resize(b"not an image at all", 64, 50).is_none());
        assert!(resize(&jpg[..20], 64, 50).is_none());
    }
}
