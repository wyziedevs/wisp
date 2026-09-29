//! Form bodies, both kinds HTML forms send: urlencoded (the default) and
//! multipart (`enctype="multipart/form-data"`, which can carry files).
//! Nothing is parsed up front: a lookup walks the body, borrowing from it.

use crate::cx::decode;
use crate::{Error, Result};
use std::borrow::Cow;

/// A form body. `Copy`, borrows the request, decodes on lookup.
#[derive(Clone, Copy)]
pub struct Form<'a> {
    body: &'a [u8],
    /// Set for a multipart body.
    boundary: Option<&'a [u8]>,
}

/// A file sent with a form: `<input type="file" name="photo">` in a form
/// with `enctype="multipart/form-data"`. Read it with `cx.form().file("photo")`.
#[derive(Clone, Debug)]
pub struct File<'a> {
    /// The file's name on the visitor's machine, as their browser sent it.
    /// It is visitor input: never use it as a path.
    pub name: Cow<'a, str>,
    /// As the browser sent it; `application/octet-stream` when it sent none.
    /// Visitor input too: check the bytes if the type matters.
    pub content_type: &'a str,
    pub bytes: &'a [u8],
}

impl<'a> Form<'a> {
    pub(crate) fn new(content_type: Option<&str>, body: &'a [u8]) -> Form<'a> {
        let none = Form {
            body: &[],
            boundary: None,
        };
        let Some(ct) = content_type else { return none };
        let (mime, params) = ct.split_once(';').unwrap_or((ct, ""));
        let mime = mime.trim();
        if mime.eq_ignore_ascii_case("application/x-www-form-urlencoded") {
            return Form {
                body,
                boundary: None,
            };
        }
        if !mime.eq_ignore_ascii_case("multipart/form-data") {
            return none;
        }
        // The boundary is found in the header but used on the body, so it is
        // taken from the body: the same bytes, borrowed for as long.
        let Some(boundary) = param(params, "boundary") else {
            return none;
        };
        let boundary = boundary.trim_matches('"').as_bytes();
        if boundary.is_empty() || boundary.len() > 70 {
            return none;
        }
        match find(body, boundary, 0) {
            Some(at) => Form {
                body,
                boundary: Some(&body[at..at + boundary.len()]),
            },
            None => none,
        }
    }

    pub fn get(&self, name: &str) -> Option<Cow<'a, str>> {
        self.iter().find(|(k, _)| k == name).map(|(_, v)| v)
    }

    /// Like `get`, but a missing field is a 400 error.
    pub fn required(&self, name: &str) -> Result<Cow<'a, str>> {
        self.get(name)
            .ok_or_else(|| Error::new(400, format!("missing form field `{name}`")))
    }

    /// A field parsed as any `FromStr` type. Missing or unparsable is a 400
    /// error that says why: `let id: i64 = cx.form().parse("id")?;`
    pub fn parse<T: std::str::FromStr<Err: std::fmt::Display>>(&self, name: &str) -> Result<T> {
        self.required(name)?
            .parse()
            .map_err(|e| Error::new(400, format!("form field `{name}`: {e}")))
    }

    /// Every value of a repeated field, like checkboxes with the same name.
    pub fn all<'n>(&self, name: &'n str) -> impl Iterator<Item = Cow<'a, str>> + 'n
    where
        'a: 'n,
    {
        self.iter().filter(move |(k, _)| k == name).map(|(_, v)| v)
    }

    /// Every field that is not a file, in order, as `(name, value)`.
    pub fn iter(&self) -> impl Iterator<Item = (Cow<'a, str>, Cow<'a, str>)> + 'a {
        let urlencoded = if self.boundary.is_none() {
            self.body
        } else {
            &[]
        };
        let fields = self
            .parts()
            .filter(|p| p.filename.is_none())
            .map(|p| (p.name, String::from_utf8_lossy(p.data)));
        pairs(urlencoded).chain(fields)
    }

    /// The file sent in field `name`, if one was chosen. A file input left
    /// empty sends no file.
    pub fn file(&self, name: &str) -> Option<File<'a>> {
        self.files(name).next()
    }

    /// Every file sent in field `name`: `<input type="file" multiple>`.
    pub fn files<'n>(&self, name: &'n str) -> impl Iterator<Item = File<'a>> + 'n
    where
        'a: 'n,
    {
        self.parts()
            .filter(move |p| p.name == name)
            .filter_map(|p| {
                let filename = p.filename?;
                if filename.is_empty() && p.data.is_empty() {
                    return None;
                }
                Some(File {
                    name: filename,
                    content_type: p.content_type.unwrap_or("application/octet-stream"),
                    bytes: p.data,
                })
            })
    }

    fn parts(&self) -> Parts<'a> {
        match self.boundary {
            Some(boundary) => Parts {
                rest: self.body,
                boundary,
                started: false,
            },
            None => Parts {
                rest: &[],
                boundary: &[],
                started: true,
            },
        }
    }
}

/// `a=1&b=x%20y` → decoded pairs. `+` is a space, as in HTML forms.
pub(crate) fn pairs(s: &[u8]) -> impl Iterator<Item = (Cow<'_, str>, Cow<'_, str>)> {
    s.split(|&b| b == b'&')
        .filter(|kv| !kv.is_empty())
        .map(|kv| {
            let (k, v) = match kv.iter().position(|&b| b == b'=') {
                Some(i) => (&kv[..i], &kv[i + 1..]),
                None => (kv, &[][..]),
            };
            (decode(k, true), decode(v, true))
        })
}

/// A header parameter's value: `boundary` in `multipart/form-data; boundary=x`.
fn param<'h>(params: &'h str, name: &str) -> Option<&'h str> {
    params.split(';').find_map(|p| {
        let (k, v) = p.split_once('=')?;
        k.trim().eq_ignore_ascii_case(name).then(|| v.trim())
    })
}

/// One part of a multipart body.
struct Part<'a> {
    name: Cow<'a, str>,
    filename: Option<Cow<'a, str>>,
    content_type: Option<&'a str>,
    data: &'a [u8],
}

/// The parts of a multipart body (RFC 7578). A body that stops making sense
/// ends the parts there, rather than failing the request.
struct Parts<'a> {
    /// From the boundary of the next part (or the preamble, before the first).
    rest: &'a [u8],
    boundary: &'a [u8],
    started: bool,
}

impl<'a> Iterator for Parts<'a> {
    type Item = Part<'a>;

    fn next(&mut self) -> Option<Part<'a>> {
        loop {
            let part = self.step();
            if part.is_none() {
                self.rest = &[];
            }
            match part? {
                Some(p) => return Some(p),
                None => continue, // a part without a name
            }
        }
    }
}

impl<'a> Parts<'a> {
    /// The next part: `None` at the end, `Some(None)` for a part to skip.
    fn step(&mut self) -> Option<Option<Part<'a>>> {
        let b = self.rest;
        // `rest` starts at a delimiter, `--boundary`, except before the first
        // part, where anything may come first.
        let at = if self.started {
            0
        } else {
            find(b, self.boundary, 0)? - 2
        };
        self.started = true;
        let after = at + 2 + self.boundary.len();
        let tail = b.get(after..)?;
        if tail.starts_with(b"--") {
            return None; // the closing delimiter
        }
        let line = tail.iter().position(|&c| c != b' ' && c != b'\t')?;
        let head = tail[line..].strip_prefix(b"\r\n")?;
        let head_len = if head.starts_with(b"\r\n") {
            0
        } else {
            head.windows(4).position(|w| w == b"\r\n\r\n")? + 2
        };
        let data_start = head_len + 2;
        // The content ends at the CRLF before the next delimiter.
        let end = find(head, self.boundary, data_start)?
            .checked_sub(4)
            .filter(|&e| e >= data_start && head[e..].starts_with(b"\r\n"))?;
        let data = &head[data_start..end];
        self.rest = &head[end + 2..];

        let (mut name, mut filename, mut content_type) = (None, None, None);
        for line in head[..head_len].split(|&c| c == b'\n') {
            let line = line.strip_suffix(b"\r").unwrap_or(line);
            let Some(colon) = line.iter().position(|&c| c == b':') else {
                continue;
            };
            let (k, v) = (&line[..colon], line[colon + 1..].trim_ascii());
            if k.eq_ignore_ascii_case(b"content-disposition") {
                (name, filename) = disposition(v);
            } else if k.eq_ignore_ascii_case(b"content-type") {
                content_type = std::str::from_utf8(v).ok();
            }
        }
        Some(name.map(|name| Part {
            name,
            filename,
            content_type,
            data,
        }))
    }
}

/// `form-data; name="field"; filename="a.png"` → the name and file name.
/// Browsers write `"` in either as `%22`, and a line break as `%0A`.
fn disposition(v: &[u8]) -> (Option<Cow<'_, str>>, Option<Cow<'_, str>>) {
    let (mut name, mut filename) = (None, None);
    let mut i = v.iter().position(|&c| c == b';').unwrap_or(v.len());
    while i < v.len() {
        i += 1; // past `;`
        while v.get(i).is_some_and(|c| c.is_ascii_whitespace()) {
            i += 1;
        }
        let key_end = i + v[i..]
            .iter()
            .position(|&c| c == b'=' || c == b';')
            .unwrap_or(v.len() - i);
        let key = v[i..key_end].trim_ascii();
        if v.get(key_end) != Some(&b'=') {
            i = key_end;
            continue;
        }
        i = key_end + 1;
        let value: &[u8];
        if v.get(i) == Some(&b'"') {
            let start = i + 1;
            let mut j = start;
            while j < v.len() && v[j] != b'"' {
                j += if v[j] == b'\\' { 2 } else { 1 };
            }
            value = &v[start..j.min(v.len())];
            i = v[j.min(v.len())..]
                .iter()
                .position(|&c| c == b';')
                .map_or(v.len(), |p| j + p);
        } else {
            let end = i + v[i..]
                .iter()
                .position(|&c| c == b';')
                .unwrap_or(v.len() - i);
            value = v[i..end].trim_ascii();
            i = end;
        }
        if key.eq_ignore_ascii_case(b"name") {
            name = Some(unquote(value));
        } else if key.eq_ignore_ascii_case(b"filename") {
            filename = Some(unquote(value));
        }
    }
    (name, filename)
}

/// A quoted string's text: `\x` is `x`.
fn unquote(value: &[u8]) -> Cow<'_, str> {
    if !value.contains(&b'\\') {
        return String::from_utf8_lossy(value);
    }
    let mut out = Vec::with_capacity(value.len());
    let mut k = 0;
    while k < value.len() {
        if value[k] == b'\\' && k + 1 < value.len() {
            k += 1;
        }
        out.push(value[k]);
        k += 1;
    }
    Cow::Owned(String::from_utf8_lossy(&out).into_owned())
}

/// The index of the first `\r\n--boundary` at or after `from`, as the index
/// of `boundary`; a body may also begin with `--boundary`. A delimiter is
/// followed by `--`, spaces or a line break (RFC 2046), so a boundary that
/// only starts some longer text is not one.
fn find(b: &[u8], boundary: &[u8], from: usize) -> Option<usize> {
    let ends = |at: usize| {
        let after = &b[at + boundary.len()..];
        after.is_empty() || after.starts_with(b"--") || matches!(after[0], b'\r' | b' ' | b'\t')
    };
    if from == 0 && b.starts_with(b"--") && b[2..].starts_with(boundary) && ends(2) {
        return Some(2);
    }
    let mut i = from;
    while i + 4 + boundary.len() <= b.len() {
        let cr = i + b[i..].iter().position(|&c| c == b'\r')?;
        if b[cr..].starts_with(b"\r\n--") && b[cr + 4..].starts_with(boundary) && ends(cr + 4) {
            return Some(cr + 4);
        }
        i = cr + 1;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    const URLENCODED: Option<&str> = Some("application/x-www-form-urlencoded");

    #[test]
    fn urlencoded() {
        let f = Form::new(URLENCODED, b"title=Hello+world&tag=a&tag=b&empty=&flag");
        assert_eq!(f.get("title").unwrap(), "Hello world");
        assert_eq!(f.get("empty").unwrap(), "");
        assert_eq!(f.get("flag").unwrap(), "");
        assert!(f.get("nope").is_none());
        assert_eq!(f.all("tag").collect::<Vec<_>>(), ["a", "b"]);
        assert_eq!(f.required("nope").unwrap_err().status(), 400);
        assert!(f.file("title").is_none());
        let f = Form::new(
            Some("application/x-www-form-urlencoded; charset=UTF-8"),
            b"id=7&bad=x",
        );
        assert_eq!(f.parse::<i64>("id").unwrap(), 7);
        assert_eq!(
            f.parse::<i64>("bad").unwrap_err().message(),
            "form field `bad`: invalid digit found in string"
        );
        assert_eq!(f.parse::<i64>("nope").unwrap_err().status(), 400);
        assert!(Form::new(Some("text/plain"), b"a=1").get("a").is_none());
        assert!(Form::new(None, b"a=1").get("a").is_none());
    }

    const BODY: &[u8] = b"preamble\r\n--XyZ\r\n\
Content-Disposition: form-data; name=\"title\"\r\n\r\n\
Hello\r\nworld\r\n--XyZ\r\n\
Content-Disposition: form-data; name=\"photo\"; filename=\"a \\\"b\\\".png\"\r\nContent-Type: image/png\r\n\r\n\
\x89PNG\r\n--XyZnot-a-boundary\r\n\r\n--XyZ\r\n\
Content-Disposition: form-data; name=\"photo\"; filename=\"\"\r\nContent-Type: application/octet-stream\r\n\r\n\
\r\n--XyZ\r\n\
Content-Disposition: form-data; name=\"tag\"\r\n\r\n\
a\r\n--XyZ\r\n\
Content-Disposition: form-data; name=\"tag\"\r\n\r\n\
b\r\n--XyZ--\r\nepilogue";

    #[test]
    fn multipart() {
        let f = Form::new(Some("multipart/form-data; boundary=\"XyZ\""), BODY);
        assert_eq!(f.get("title").unwrap(), "Hello\r\nworld");
        assert_eq!(f.all("tag").collect::<Vec<_>>(), ["a", "b"]);
        assert!(f.get("photo").is_none(), "a file is not a text field");
        let photo = f.file("photo").unwrap();
        assert_eq!(
            (photo.name.as_ref(), photo.content_type),
            ("a \"b\".png", "image/png")
        );
        assert_eq!(photo.bytes, b"\x89PNG\r\n--XyZnot-a-boundary\r\n");
        assert_eq!(
            f.files("photo").count(),
            1,
            "an empty file input sends no file"
        );
        assert_eq!(f.iter().count(), 3);
        assert!(f.file("title").is_none());
    }

    #[test]
    fn broken_multipart_ends_early() {
        let ct = Some("multipart/form-data; boundary=XyZ");
        for cut in 0..BODY.len() {
            let f = Form::new(ct, &BODY[..cut]);
            let _ = (f.iter().count(), f.files("photo").count());
        }
        let no_head_end = b"--XyZ\r\nContent-Disposition: form-data; name=\"a\"\r\n1\r\n--XyZ--";
        assert!(Form::new(ct, no_head_end).get("a").is_none());
        let no_name = b"--XyZ\r\nContent-Type: text/plain\r\n\r\nx\r\n--XyZ\r\nContent-Disposition: form-data; name=b\r\n\r\n2\r\n--XyZ--";
        assert_eq!(Form::new(ct, no_name).get("b").unwrap(), "2");
        assert!(
            Form::new(Some("multipart/form-data"), BODY)
                .get("title")
                .is_none(),
            "no boundary"
        );
        assert!(
            Form::new(Some("multipart/form-data; boundary=other"), BODY)
                .get("title")
                .is_none()
        );
    }
}
