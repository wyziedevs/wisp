//! Images in templates. `<img src="$lib/photo.jpg">` (a file of `src/lib`)
//! or `<img src="/photo.jpg">` (one of `static/`), a JPEG, PNG or WebP,
//! gets `width` and `height` from the file's header, so the page does not
//! shift as it loads. A release build also gets `srcset` (the WebP widths
//! `wisp build` wrote into `.wisp/img` with cwebp, served from `/_app/img/`
//! as immutable), `sizes`, `loading="lazy"` and `decoding="async"`. What
//! the tag already has stays; `data-wisp-raw` keeps it as written (but a
//! `$lib/` src, which has no URL of its own).

use crate::codegen::encode_path;
use crate::fnv1a;
use crate::protocol::IMAGES;
use std::borrow::Cow;
use std::fmt::Write as _;
use std::fs;
use std::io::Read;
use std::ops::Range;
use std::path::{Path, PathBuf};

/// Where `wisp build` writes the WebP widths, named by content hash.
pub const DIR: &str = ".wisp/img";

/// The widths written, each at most the image's own.
const WIDTHS: [u32; 3] = [640, 1280, 1920];

/// The attribute that keeps a tag as written.
const RAW: &str = "data-wisp-raw";

/// An image's size as shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Size {
    pub width: u32,
    pub height: u32,
    /// Its EXIF orientation turns or flips it: the size is as shown
    /// (turned), but cwebp would not turn it, so it gets no WebP.
    pub turned: bool,
}

/// The size in a JPEG, PNG or WebP header.
pub fn size(b: &[u8]) -> Option<Size> {
    let (width, height, turned) = match b.get(..4)? {
        [0x89, b'P', b'N', b'G'] => png(b)?,
        [0xff, 0xd8, ..] => jpeg(b)?,
        b"RIFF" => webp(b)?,
        _ => return None,
    };
    (width > 0 && height > 0).then_some(Size {
        width,
        height,
        turned,
    })
}

fn be16(b: &[u8], i: usize) -> Option<u32> {
    Some(u32::from(u16::from_be_bytes(
        b.get(i..i + 2)?.try_into().ok()?,
    )))
}

fn be32(b: &[u8], i: usize) -> Option<u32> {
    Some(u32::from_be_bytes(b.get(i..i + 4)?.try_into().ok()?))
}

fn le(b: &[u8], i: usize, n: usize) -> Option<u32> {
    let bytes = b.get(i..i + n)?;
    Some((bytes.iter().rev()).fold(0, |v, &x| v << 8 | u32::from(x)))
}

fn png(b: &[u8]) -> Option<(u32, u32, bool)> {
    if b.get(..8)? != b"\x89PNG\r\n\x1a\n" || b.get(12..16)? != b"IHDR" {
        return None;
    }
    Some((be32(b, 16)?, be32(b, 20)?, false))
}

/// The first frame's size, from its SOF segment; turned by the EXIF
/// orientation of an APP1 segment before it.
fn jpeg(b: &[u8]) -> Option<(u32, u32, bool)> {
    let mut i = 2;
    let mut orient = 1;
    loop {
        if *b.get(i)? != 0xff {
            return None;
        }
        // Any number of 0xff may pad a marker.
        while *b.get(i + 1)? == 0xff {
            i += 1;
        }
        let m = b[i + 1];
        i += 2;
        if m == 0x01 || (0xd0..=0xd8).contains(&m) {
            continue;
        }
        let len = be16(b, i)? as usize;
        match m {
            0xe1 => orient = b.get(i + 2..i + len).and_then(exif).unwrap_or(orient),
            0xc0..=0xcf if !matches!(m, 0xc4 | 0xc8 | 0xcc) => {
                let (h, w) = (be16(b, i + 3)?, be16(b, i + 5)?);
                let turned = orient != 1;
                return Some(if orient >= 5 {
                    (h, w, turned)
                } else {
                    (w, h, turned)
                });
            }
            0xd9 | 0xda => return None,
            _ => {}
        }
        i += len.max(2);
    }
}

/// The orientation tag of an `Exif` segment's first directory.
fn exif(seg: &[u8]) -> Option<u32> {
    let tiff = seg.strip_prefix(b"Exif\0\0")?;
    let little = tiff.get(..2)? == b"II";
    let num = |i: usize, n: usize| {
        if little {
            le(tiff, i, n)
        } else if n == 2 {
            be16(tiff, i)
        } else {
            be32(tiff, i)
        }
    };
    let dir = num(4, 4)? as usize;
    (0..num(dir, 2)? as usize)
        .map(|k| dir + 2 + 12 * k)
        .find(|&e| num(e, 2) == Some(0x0112))
        .and_then(|e| num(e + 8, 2))
}

fn webp(b: &[u8]) -> Option<(u32, u32, bool)> {
    if b.get(8..12)? != b"WEBP" {
        return None;
    }
    let (w, h) = match b.get(12..16)? {
        b"VP8 " if b.get(23..26)? == [0x9d, 0x01, 0x2a] => {
            (le(b, 26, 2)? & 0x3fff, le(b, 28, 2)? & 0x3fff)
        }
        b"VP8L" if *b.get(20)? == 0x2f => {
            let bits = le(b, 21, 4)?;
            ((bits & 0x3fff) + 1, ((bits >> 14) & 0x3fff) + 1)
        }
        b"VP8X" => (le(b, 24, 3)? + 1, le(b, 27, 3)? + 1),
        _ => return None,
    };
    Some((w, h, false))
}

/// The WebP widths of an image `width` wide: never wider than it.
pub fn widths(width: u32) -> Vec<u32> {
    let mut out: Vec<u32> = WIDTHS.iter().map(|&w| w.min(width)).collect();
    out.dedup();
    out
}

/// An image's content hash, which names its files.
pub fn hash(bytes: &[u8]) -> String {
    format!("{:016x}", fnv1a(bytes))
}

/// The file of a WebP width in `DIR`, and its URL after `IMAGES`.
pub fn webp_name(hash: &str, width: u32) -> String {
    format!("{hash}-{width}.webp")
}

/// The URL of a `src/lib` image's original in a release build.
pub fn lib_url(hash: &str, file: &Path) -> String {
    format!("{IMAGES}{hash}.{}", ext(file))
}

fn ext(file: &Path) -> String {
    (file.extension())
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default()
}

/// The file an image `src` names: `$lib/x` in `src/lib` (true), `/x` in
/// `static/`. A JPEG, PNG or WebP by its name, or none.
fn source(root: &Path, src: &str) -> Option<(PathBuf, bool)> {
    let (dir, rest, lib) = match src.strip_prefix("$lib/") {
        Some(rest) => ("src/lib", rest, true),
        None => (
            "static",
            src.strip_prefix('/').filter(|r| !r.starts_with('/'))?,
            false,
        ),
    };
    let ok = !rest.is_empty()
        && !rest.contains(['?', '#', '\\', '{', ':'])
        && rest
            .split('/')
            .all(|s| !s.is_empty() && s != "." && s != "..");
    let file = root.join(dir).join(rest);
    (ok && matches!(ext(&file).as_str(), "jpg" | "jpeg" | "png" | "webp")).then_some((file, lib))
}

/// An image a template shows: its file, whether it is `src/lib`'s, and
/// whether a tag without `data-wisp-raw` shows it (it gets WebP widths).
#[derive(Debug, PartialEq)]
pub struct Found {
    pub file: PathBuf,
    pub lib: bool,
    pub webp: bool,
}

/// Every image the app's templates show, each once, in file order: for
/// `wisp build` to encode, and a release build to embed.
pub fn sources(root: &Path) -> Vec<Found> {
    let mut files = Vec::new();
    for dir in ["routes", "components"] {
        crate::wisp_files(&root.join("src").join(dir), dir == "routes", &mut files, 0);
    }
    files.retain(|f| !f.to_string_lossy().ends_with(".stories.wisp"));
    files.sort();
    let mut out: Vec<Found> = Vec::new();
    for f in files {
        let Ok(src) = crate::read_source(&f) else {
            continue;
        };
        let markup = crate::split_front(&src).map_or(src, |(_, m)| m);
        for img in imgs(&markup) {
            let Some((file, lib)) = img.src(&markup).and_then(|s| source(root, s)) else {
                continue;
            };
            let webp = !img.has(RAW);
            match out.iter_mut().find(|f| f.file == file) {
                Some(f) => f.webp |= webp,
                None => out.push(Found { file, lib, webp }),
            }
        }
    }
    out
}

/// The markup with each local image's tag filled in (see the top). A
/// `$lib/` file that is not there is an error, `line: msg`. Lines stay
/// where they were.
pub(crate) fn rewrite<'m>(
    markup: &'m str,
    root: &Path,
    release: bool,
) -> Result<Cow<'m, str>, String> {
    if !(markup.as_bytes().windows(4)).any(|w| w.eq_ignore_ascii_case(b"<img")) {
        return Ok(Cow::Borrowed(markup));
    }
    let mut edits = Vec::new();
    for img in imgs(markup) {
        img.plan(markup, root, release, &mut edits)?;
    }
    if edits.is_empty() {
        return Ok(Cow::Borrowed(markup));
    }
    edits.sort_by_key(|e: &(Range<usize>, String)| e.0.start);
    let mut out = String::with_capacity(markup.len() + 200 * edits.len());
    let mut at = 0;
    for (range, text) in edits {
        out.push_str(&markup[at..range.start]);
        out.push_str(&text);
        at = range.end;
    }
    out.push_str(&markup[at..]);
    Ok(Cow::Owned(out))
}

/// An attribute: its lowercase name, where it is, and its value's text
/// when quoted.
struct Attr {
    name: String,
    at: Range<usize>,
    value: Option<Range<usize>>,
}

/// An `<img>` tag: its attributes, and where new ones go (after the last).
struct Img {
    start: usize,
    attrs: Vec<Attr>,
    end: usize,
}

impl Img {
    fn get(&self, name: &str) -> Option<&Attr> {
        self.attrs.iter().find(|a| a.name == name)
    }

    fn has(&self, name: &str) -> bool {
        self.get(name).is_some()
    }

    /// Its `src`, when that is quoted text with no hole.
    fn src<'m>(&self, m: &'m str) -> Option<&'m str> {
        let v = &m[self.get("src")?.value.clone()?];
        (!v.contains('{')).then_some(v)
    }

    /// The edits that fill it in.
    fn plan(
        &self,
        m: &str,
        root: &Path,
        release: bool,
        edits: &mut Vec<(Range<usize>, String)>,
    ) -> Result<(), String> {
        let raw = self.get(RAW);
        if let Some(a) = raw {
            edits.push((a.at.clone(), String::new()));
        }
        let Some(v) = self.get("src").and_then(|a| a.value.clone()) else {
            return Ok(());
        };
        let src = &m[v.clone()];
        let Some((file, lib)) = source(root, src).filter(|_| !src.contains('{')) else {
            return Ok(());
        };
        let read = if release {
            fs::read(&file).map(|b| (size(&b), Some(hash(&b))))
        } else {
            head_size(&file).map(|s| (s, None))
        };
        let (size, hash) = match read {
            Ok(r) => r,
            Err(_) if lib => {
                let line = m[..self.start].matches('\n').count() + 1;
                return Err(format!(
                    "{line}: there is no src/lib/{} for this <img>",
                    &src[5..]
                ));
            }
            Err(_) => return Ok(()),
        };
        if lib {
            let url = match &hash {
                Some(h) => lib_url(h, &file),
                None => format!("{IMAGES}lib/{}", encode_path(&src[5..])),
            };
            edits.push((v, url));
        }
        if raw.is_some() {
            return Ok(());
        }
        let mut add = String::new();
        let mut put = |name: &str, value: &str| {
            if !self.has(name) {
                let _ = write!(add, " {name}=\"{value}\"");
            }
        };
        if let Some(s) = size
            && !self.has("width")
            && !self.has("height")
        {
            put("width", &s.width.to_string());
            put("height", &s.height.to_string());
        }
        if let (Some(h), Some(s)) = (&hash, size)
            && !s.turned
        {
            let ws = widths(s.width);
            let dir = root.join(DIR);
            if ws.iter().all(|&w| dir.join(webp_name(h, w)).is_file()) {
                let set: Vec<String> = (ws.iter())
                    .map(|&w| format!("{IMAGES}{} {w}w", webp_name(h, w)))
                    .collect();
                if !self.has("srcset") {
                    put("srcset", &set.join(", "));
                    put("sizes", "100vw");
                }
            }
        }
        if release {
            put("loading", "lazy");
            put("decoding", "async");
        }
        if !add.is_empty() {
            edits.push((self.end..self.end, add));
        }
        Ok(())
    }
}

/// The size from the first 64 KiB, or the whole file when its header is
/// further in (a JPEG's EXIF thumbnail may come first).
fn head_size(file: &Path) -> std::io::Result<Option<Size>> {
    let mut head = Vec::new();
    fs::File::open(file)?.take(1 << 16).read_to_end(&mut head)?;
    if let Some(s) = size(&head) {
        return Ok(Some(s));
    }
    let all = fs::read(file)?;
    Ok((all.len() > head.len()).then(|| size(&all)).flatten())
}

/// The `<img>` tags of markup, outside comments, `<script>`, `<style>` and
/// `{…}` holes.
fn imgs(m: &str) -> Vec<Img> {
    let b = m.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'{' => i = braces(b, i),
            b'<' if b[i..].starts_with(b"<!--") => {
                i = find(b, i + 4, b"-->").map_or(b.len(), |k| k + 3);
            }
            b'<' if b.get(i + 1).is_some_and(u8::is_ascii_alphabetic) => {
                let start = i;
                let name_end = i
                    + 1
                    + (b[i + 1..].iter())
                        .take_while(|c| c.is_ascii_alphanumeric() || matches!(c, b'-' | b':'))
                        .count();
                let name = m[i + 1..name_end].to_ascii_lowercase();
                let (attrs, end, next) = attrs(m, name_end);
                i = next;
                match name.as_str() {
                    "img" => out.push(Img { start, attrs, end }),
                    "script" | "style" => {
                        let close = format!("</{name}");
                        i = (b[i..].windows(close.len()))
                            .position(|w| w.eq_ignore_ascii_case(close.as_bytes()))
                            .map_or(b.len(), |k| i + k);
                    }
                    _ => {}
                }
            }
            _ => i += 1,
        }
    }
    out
}

/// A tag's attributes from `i` (after its name), where the last ends, and
/// where the tag does.
fn attrs(m: &str, mut i: usize) -> (Vec<Attr>, usize, usize) {
    let b = m.as_bytes();
    let mut out = Vec::new();
    let mut last = i;
    loop {
        while b.get(i).is_some_and(u8::is_ascii_whitespace) {
            i += 1;
        }
        match b.get(i) {
            None => return (out, last, i),
            Some(b'>') => return (out, last, i + 1),
            Some(b'/') if b.get(i + 1) == Some(&b'>') => return (out, last, i + 2),
            Some(b'{') => {
                i = braces(b, i);
                last = i;
                continue;
            }
            _ => {}
        }
        let start = i;
        while b.get(i).is_some_and(|&c| {
            !c.is_ascii_whitespace()
                && c != b'='
                && c != b'>'
                && !(c == b'/' && b.get(i + 1) == Some(&b'>'))
        }) {
            i += 1;
        }
        // A stray `/` or `=`: step over it.
        if i == start {
            i += 1;
            continue;
        }
        let name = m[start..i].to_ascii_lowercase();
        let mut j = i;
        while b.get(j).is_some_and(u8::is_ascii_whitespace) {
            j += 1;
        }
        let mut value = None;
        if b.get(j) == Some(&b'=') {
            j += 1;
            while b.get(j).is_some_and(u8::is_ascii_whitespace) {
                j += 1;
            }
            match b.get(j) {
                Some(&q @ (b'"' | b'\'')) => {
                    let mut k = j + 1;
                    while k < b.len() && b[k] != q {
                        k = if b[k] == b'{' { braces(b, k) } else { k + 1 };
                    }
                    value = Some(j + 1..k.min(b.len()));
                    i = (k + 1).min(b.len());
                }
                Some(b'{') => i = braces(b, j),
                _ => {
                    i = j;
                    while b
                        .get(i)
                        .is_some_and(|c| !c.is_ascii_whitespace() && *c != b'>')
                    {
                        i += 1;
                    }
                }
            }
        }
        out.push(Attr {
            name,
            at: start..i,
            value,
        });
        last = i;
    }
}

/// After the `}` that closes the `{` at `i` (strings inside skipped).
fn braces(b: &[u8], mut i: usize) -> usize {
    let mut depth = 0;
    while i < b.len() {
        match b[i] {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return i + 1;
                }
            }
            b'"' => {
                i += 1;
                while i < b.len() && b[i] != b'"' {
                    i += if b[i] == b'\\' { 2 } else { 1 };
                }
            }
            _ => {}
        }
        i += 1;
    }
    b.len()
}

fn find(b: &[u8], from: usize, what: &[u8]) -> Option<usize> {
    (b.get(from..)?.windows(what.len()))
        .position(|w| w == what)
        .map(|k| from + k)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png_of(w: u32, h: u32) -> Vec<u8> {
        let mut b = b"\x89PNG\r\n\x1a\n\0\0\0\x0dIHDR".to_vec();
        b.extend(w.to_be_bytes());
        b.extend(h.to_be_bytes());
        b.extend([8, 2, 0, 0, 0]);
        b
    }

    /// A JPEG header: an EXIF segment with this orientation (when some),
    /// then a frame of this size.
    fn jpeg_of(w: u16, h: u16, orient: Option<u16>) -> Vec<u8> {
        let mut b = vec![0xff, 0xd8, 0xff, 0xe0, 0, 4, 0, 0];
        if let Some(o) = orient {
            let mut tiff = b"MM\0\x2a\0\0\0\x08\0\x01\x01\x12\0\x03\0\0\0\x01".to_vec();
            tiff.extend(o.to_be_bytes());
            tiff.extend([0, 0, 0, 0, 0, 0]);
            let seg = [b"Exif\0\0".as_slice(), &tiff].concat();
            b.extend([0xff, 0xe1]);
            b.extend((seg.len() as u16 + 2).to_be_bytes());
            b.extend(seg);
        }
        b.extend([0xff, 0xff, 0xc2, 0, 11, 8]);
        b.extend(h.to_be_bytes());
        b.extend(w.to_be_bytes());
        b.extend([1, 1, 0x11, 0]);
        b
    }

    fn sized(width: u32, height: u32, turned: bool) -> Option<Size> {
        Some(Size {
            width,
            height,
            turned,
        })
    }

    #[test]
    fn sizes_from_headers() {
        assert_eq!(size(&png_of(800, 600)), sized(800, 600, false));
        assert_eq!(size(&jpeg_of(1024, 768, None)), sized(1024, 768, false));
        assert_eq!(size(&jpeg_of(1024, 768, Some(1))), sized(1024, 768, false));
        // Turned a quarter: shown as 768 wide.
        assert_eq!(size(&jpeg_of(1024, 768, Some(6))), sized(768, 1024, true));
        // Upside down: the same size, but cwebp would not turn it.
        assert_eq!(size(&jpeg_of(1024, 768, Some(3))), sized(1024, 768, true));
        let mut lossy = b"RIFF\0\0\0\0WEBPVP8 \0\0\0\0\0\0\0\x9d\x01\x2a".to_vec();
        lossy.extend([0x20, 0x03, 0x58, 0x02]);
        assert_eq!(size(&lossy), sized(800, 600, false));
        let mut lossless = b"RIFF\0\0\0\0WEBPVP8L\0\0\0\0\x2f".to_vec();
        lossless.extend(((799u32) | (599 << 14)).to_le_bytes());
        assert_eq!(size(&lossless), sized(800, 600, false));
        let mut extended = b"RIFF\0\0\0\0WEBPVP8X\0\0\0\0\0\0\0\0".to_vec();
        extended.extend([0x1f, 0x03, 0, 0x57, 0x02, 0]);
        assert_eq!(size(&extended), sized(800, 600, false));
        // Cut short, or not an image.
        assert_eq!(size(&png_of(800, 600)[..20]), None);
        assert_eq!(size(&jpeg_of(10, 10, None)[..12]), None);
        assert_eq!(size(b"GIF89a"), None);
        assert_eq!(size(&png_of(0, 600)), None);
    }

    #[test]
    fn widths_never_grow() {
        assert_eq!(widths(3000), [640, 1280, 1920]);
        assert_eq!(widths(1000), [640, 1000]);
        assert_eq!(widths(300), [300]);
    }

    fn app(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("wisp-img-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("src/lib")).unwrap();
        fs::create_dir_all(root.join("static/pics")).unwrap();
        fs::write(root.join("src/lib/cat.png"), png_of(1000, 500)).unwrap();
        fs::write(root.join("static/pics/dog.jpg"), jpeg_of(300, 200, None)).unwrap();
        root
    }

    #[test]
    fn tags_are_filled_in() {
        let root = app("tags");
        let markup = "<p>{a < b}</p>\n<img src=\"$lib/cat.png\" alt={name} class=\"x\">\n\
                      <IMG alt=\"d\" src='/pics/dog.jpg' width=\"30\" height=\"20\" loading=\"eager\"/>\n\
                      <img src=\"https://x.com/a.png\" alt=\"\">\n<img src=\"/pics/dog.jpg\" data-wisp-raw alt=\"\">\n\
                      <!-- <img src=\"$lib/gone.png\"> --><script>let s = '<img src=\"$lib/gone.png\">'</script>";
        let dev = rewrite(markup, &root, false).unwrap();
        assert_eq!(
            dev,
            "<p>{a < b}</p>\n<img src=\"/_app/img/lib/cat.png\" alt={name} class=\"x\" width=\"1000\" height=\"500\">\n\
             <IMG alt=\"d\" src='/pics/dog.jpg' width=\"30\" height=\"20\" loading=\"eager\"/>\n\
             <img src=\"https://x.com/a.png\" alt=\"\">\n<img src=\"/pics/dog.jpg\"  alt=\"\">\n\
             <!-- <img src=\"$lib/gone.png\"> --><script>let s = '<img src=\"$lib/gone.png\">'</script>"
        );
        assert_eq!(dev.lines().count(), markup.lines().count());

        // A release build: hashed, and the WebP widths once they are there.
        let h = hash(&png_of(1000, 500));
        let release = rewrite("<img src=\"$lib/cat.png\" alt=\"c\">", &root, true).unwrap();
        assert_eq!(
            release,
            format!(
                "<img src=\"/_app/img/{h}.png\" alt=\"c\" width=\"1000\" height=\"500\" loading=\"lazy\" decoding=\"async\">"
            )
        );
        fs::create_dir_all(root.join(DIR)).unwrap();
        for w in [640, 1000] {
            fs::write(root.join(DIR).join(webp_name(&h, w)), "").unwrap();
        }
        let release = rewrite(
            "<img src=\"$lib/cat.png\" sizes=\"50vw\" alt=\"c\" />",
            &root,
            true,
        )
        .unwrap();
        assert_eq!(
            release,
            format!(
                "<img src=\"/_app/img/{h}.png\" sizes=\"50vw\" alt=\"c\" width=\"1000\" height=\"500\" \
                 srcset=\"/_app/img/{h}-640.webp 640w, /_app/img/{h}-1000.webp 1000w\" loading=\"lazy\" decoding=\"async\" />"
            )
        );

        // Nothing to do: the same text, not a copy.
        assert!(matches!(
            rewrite("<p>no images</p>", &root, true),
            Ok(Cow::Borrowed(_))
        ));
        let err = rewrite("\n\n<img src=\"$lib/gone.png\">", &root, false).unwrap_err();
        assert_eq!(err, "3: there is no src/lib/gone.png for this <img>");

        let pages = root.join("src/routes");
        fs::create_dir_all(&pages).unwrap();
        fs::write(pages.join("+page.wisp"), markup).unwrap();
        let found = sources(&root);
        let _ = fs::remove_dir_all(&root);
        assert_eq!(
            found,
            [
                Found {
                    file: root.join("src/lib").join("cat.png"),
                    lib: true,
                    webp: true
                },
                Found {
                    file: root.join("static").join("pics/dog.jpg"),
                    lib: false,
                    webp: true
                },
            ]
        );
    }
}
