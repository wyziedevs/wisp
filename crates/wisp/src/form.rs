//! Form bodies, both kinds HTML forms send: urlencoded (the default) and
//! multipart (`enctype="multipart/form-data"`, which can carry files).
//! Nothing is parsed up front: a lookup walks the body, borrowing from it.

use crate::cx::decode;
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
    /// The file's name on the visitor's machine, without its folders.
    /// It is visitor input: never use it as a path.
    pub name: Cow<'a, str>,
    /// As the browser sent it; `application/octet-stream` when it sent none.
    /// Visitor input too: check the bytes if the type matters.
    pub content_type: &'a str,
    /// The file's bytes.
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

    /// The value of field `name`, or `None` when the form has none of that name.
    pub fn get(&self, name: &str) -> Option<Cow<'a, str>> {
        self.iter().find(|(k, _)| k == name).map(|(_, v)| v)
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
                    name: if filename.is_empty() {
                        Cow::Borrowed("file")
                    } else {
                        filename
                    },
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
            filename = Some(base_name(unquote(value)));
        }
    }
    (name, filename)
}

/// A quoted string's text. Browsers write `"`, CR and LF in it as `%22`,
/// `%0D` and `%0A` (WHATWG), and only those: any other `%` is itself.
/// Older clients write `\"` and `\\`; a `\` before anything else is itself,
/// as in an old browser's `C:\photos\a.png`.
fn unquote(value: &[u8]) -> Cow<'_, str> {
    if !value.iter().any(|&c| c == b'\\' || c == b'%') {
        return String::from_utf8_lossy(value);
    }
    let mut out = Vec::with_capacity(value.len());
    let mut k = 0;
    while k < value.len() {
        let (c, n) = match (value[k], value.get(k + 1), value.get(k + 2)) {
            (b'\\', Some(&e @ (b'"' | b'\\')), _) => (e, 2),
            (b'%', Some(b'2'), Some(b'2')) => (b'"', 3),
            (b'%', Some(b'0'), Some(b'd' | b'D')) => (b'\r', 3),
            (b'%', Some(b'0'), Some(b'a' | b'A')) => (b'\n', 3),
            (c, _, _) => (c, 1),
        };
        out.push(c);
        k += n;
    }
    Cow::Owned(String::from_utf8_lossy(&out).into_owned())
}

/// A file name without the folders some clients send with it:
/// `C:\photos\a.png` and `photos/a.png` are `a.png`. A `:` ends a folder
/// too: on Windows `C:a.png` joined to a folder is the drive's own path,
/// and `a.png:x` a stream of `a.png`. Control characters are taken out,
/// and a name that is then empty, `.` or `..` is `file`, so one joined to
/// a folder stays in it. Only `""` as sent stays empty: the browser's
/// "no file chosen".
fn base_name(name: Cow<'_, str>) -> Cow<'_, str> {
    if name.is_empty() {
        return name;
    }
    let at = name.rfind(['/', '\\', ':']).map_or(0, |at| at + 1);
    let clean = !name[at..].contains(char::is_control);
    let name = match name {
        Cow::Borrowed(s) if clean => Cow::Borrowed(&s[at..]),
        _ => Cow::Owned(name[at..].chars().filter(|c| !c.is_control()).collect()),
    };
    match &*name {
        "" | "." | ".." => Cow::Borrowed("file"),
        _ => name,
    }
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
        assert!(f.file("title").is_none());
        let f = Form::new(
            Some("application/x-www-form-urlencoded; charset=UTF-8"),
            b"id=7&bad=x",
        );
        assert_eq!(f.get("id").unwrap(), "7");
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
    fn filenames_unquote_as_browsers_write_them() {
        let ct = Some("multipart/form-data; boundary=XyZ");
        let part = |disposition: &str| {
            format!("--XyZ\r\nContent-Disposition: form-data; {disposition}\r\n\r\nx\r\n--XyZ--")
        };
        for (sent, want) in [
            (r#"filename="a.png""#, "a.png"),
            (r#"filename="a%22b%22.png""#, "a\"b\".png"),
            (r#"filename="a%0D%0ab.png""#, "ab.png"),
            (r#"filename="a	b.png""#, "ab.png"),
            (r#"filename="..""#, "file"),
            (r#"filename="photos/.""#, "file"),
            (r#"filename="photos/""#, "file"),
            (r#"filename="%0D%0A""#, "file"),
            (r#"filename="""#, "file"),
            (r#"filename="100%25 %41.png""#, "100%25 %41.png"),
            (r#"filename="a%2Fb.png""#, "a%2Fb.png"),
            (r#"filename="a \"b\".png""#, "a \"b\".png"),
            (r#"filename="a\\b.png""#, "b.png"),
            (r#"filename="C:\photos\a.png""#, "a.png"),
            (r#"filename="\\server\share\a.png""#, "a.png"),
            (r#"filename="photos/2026/a.png""#, "a.png"),
            (r#"filename="../../etc/passwd""#, "passwd"),
            (r#"filename="C:evil.exe""#, "evil.exe"),
            (r#"filename="a.png:stream""#, "stream"),
            (r#"filename="C:""#, "file"),
            (r#"filename="..%00/x""#, "x"),
        ] {
            let body = part(&format!("name=\"f\"; {sent}"));
            let file = Form::new(ct, body.as_bytes()).file("f");
            assert_eq!(file.as_ref().map(|f| f.name.as_ref()), Some(want), "{sent}");
        }
        let body = part(r#"name="a%22b\"c""#);
        assert_eq!(
            Form::new(ct, body.as_bytes()).get("a\"b\"c").as_deref(),
            Some("x"),
            "a field's name is unquoted the same way"
        );
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

    /// Bodies as clients other than a browser write them: nothing panics,
    /// and each pair reads as sent. `&` separates, `=` splits once, `+` is
    /// a space, a bad `%` is itself, bad UTF-8 is U+FFFD, `a[b]` is a name.
    #[test]
    fn urlencoded_edges() {
        let pairs = |b: &'static [u8]| -> Vec<(String, String)> {
            Form::new(URLENCODED, b)
                .iter()
                .map(|(k, v)| (k.into_owned(), v.into_owned()))
                .collect()
        };
        let p = |k: &str, v: &str| (k.to_string(), v.to_string());
        assert_eq!(pairs(b""), []);
        assert_eq!(pairs(b"&&=&"), [p("", "")]);
        assert_eq!(pairs(b"a=b=c&d"), [p("a", "b=c"), p("d", "")]);
        assert_eq!(pairs(b"a=1;b=2"), [p("a", "1;b=2")], "`;` is no separator");
        assert_eq!(
            pairs(b"a[b]=1&a[]=2&a[]=3&a.b=4"),
            [p("a[b]", "1"), p("a[]", "2"), p("a[]", "3"), p("a.b", "4")]
        );
        assert_eq!(pairs(b"q=%2B+%25%zz%4%"), [p("q", "+ %%zz%4%")]);
        assert_eq!(pairs(b"u=%FF%C3%A9%00"), [p("u", "\u{FFFD}é\0")]);
        assert_eq!(pairs(b"k%3Dv=%26"), [p("k=v", "&")]);
        assert_eq!(pairs(b"a=1\r\nb=2"), [p("a", "1\r\nb=2")]);
        let f = Form::new(URLENCODED, b"x=first&x=second&x=");
        assert_eq!(f.get("x").unwrap(), "first", "the first of a repeated name");
        assert_eq!(f.all("x").count(), 3);
        assert!(f.get("X").is_none(), "names are case-sensitive");
        // A big body is read as it is: no copy of a value that needs none.
        let big: Vec<u8> = std::iter::repeat_n(b"a=b&", 300_000)
            .flatten()
            .copied()
            .collect();
        let f = Form::new(URLENCODED, &big);
        assert!(matches!(f.get("a"), Some(Cow::Borrowed("b"))));
        assert_eq!(f.all("a").count(), 300_000);
    }

    /// Multipart bodies that are not quite right: a boundary too long or
    /// missing, no closing delimiter, a part without a name or without
    /// headers, bare LF, headers in any case, and a name or filename
    /// written without quotes.
    #[test]
    fn multipart_edges() {
        let ct = |b: &str| format!("multipart/form-data; boundary={b}");
        let long = "x".repeat(71);
        assert!(
            Form::new(Some(&ct(&long)), b"--xxx\r\n\r\n")
                .get("a")
                .is_none(),
            "a boundary over 70 bytes (RFC 2046) is none"
        );
        assert!(
            Form::new(Some("multipart/form-data; boundary="), b"a=1")
                .get("a")
                .is_none()
        );
        assert!(
            Form::new(Some("multipart/form-data; boundary=\"\""), b"")
                .get("a")
                .is_none()
        );
        let ct = Some("multipart/form-data; boundary=B");
        // No closing delimiter: what is complete is read, the rest is not.
        let open = b"--B\r\ncontent-disposition: form-data; name=\"a\"\r\n\r\n1\r\n--B\r\ncontent-disposition: form-data; name=\"b\"\r\n\r\n2";
        let f = Form::new(ct, open);
        assert_eq!(f.get("a").unwrap(), "1");
        assert!(f.get("b").is_none());
        // Headers in any case, unquoted name and filename, LF-only lines.
        let loose = b"--B\r\nCONTENT-DISPOSITION: Form-Data; NAME=a; FILENAME=x.txt\r\nCONTENT-TYPE: Text/Plain\r\n\r\nhi\r\n--B--\r\n";
        let f = Form::new(ct, loose);
        let file = f.file("a").expect("unquoted name and filename");
        assert_eq!((file.name.as_ref(), file.bytes), ("x.txt", &b"hi"[..]));
        let lf = b"--B\ncontent-disposition: form-data; name=\"a\"\n\n1\n--B--\n";
        let _ = Form::new(ct, lf).iter().count();
        // A part that is only a boundary, and a body of boundaries.
        for body in [
            &b"--B\r\n--B--"[..],
            b"--B--",
            b"--B\r\n\r\n\r\n--B--",
            b"\r\n--B\r\n\r\n--B\r\n\r\n--B--",
        ] {
            let f = Form::new(ct, body);
            let _ = (f.iter().count(), f.files("a").count());
        }
        // A file part's bytes may hold `--B` not at a line start, and CRs.
        let tricky = b"--B\r\ncontent-disposition: form-data; name=\"f\"; filename=\"t\"\r\n\r\n--B\r--B\n\r\n--Bx\r\n--B--\r\n";
        let f = Form::new(ct, tricky);
        assert_eq!(f.file("f").unwrap().bytes, b"--B\r--B\n\r\n--Bx");
        // Many parts: each read, in order.
        let mut many = Vec::new();
        for i in 0..2000 {
            many.extend_from_slice(
                format!("--B\r\ncontent-disposition: form-data; name=\"n\"\r\n\r\n{i}\r\n")
                    .as_bytes(),
            );
        }
        many.extend_from_slice(b"--B--\r\n");
        let f = Form::new(ct, &many);
        assert_eq!(f.all("n").count(), 2000);
        assert_eq!(f.all("n").nth(1999).unwrap(), "1999");
    }

    use crate::fuzz::{Rng, mutate};

    /// `s` as a browser sends it in a urlencoded form.
    fn urlencode(s: &str) -> String {
        s.bytes()
            .map(|b| match b {
                b' ' => "+".to_string(),
                b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'*' => {
                    (b as char).to_string()
                }
                _ => format!("%{b:02X}"),
            })
            .collect()
    }

    #[test]
    fn urlencoded_fields_read_back() {
        let mut rng = Rng::new(10);
        for _ in 0..3000 {
            let fields: Vec<(String, String)> = (0..rng.below(6))
                .map(|i| (format!("{}{i}", rng.text(4)), rng.text(12)))
                .collect();
            let body: Vec<String> = fields
                .iter()
                .map(|(k, v)| format!("{}={}", urlencode(k), urlencode(v)))
                .collect();
            let body = body.join("&");
            let f = Form::new(URLENCODED, body.as_bytes());
            for (k, v) in &fields {
                assert_eq!(f.get(k).as_deref(), Some(v.as_str()), "{body}");
            }
            assert_eq!(f.iter().count(), fields.len());
            let mut broken = body.into_bytes();
            mutate(&mut rng, &mut broken);
            let _ = Form::new(URLENCODED, &broken).iter().count();
            assert!(decode(&broken, true).len() <= broken.len() * 3);
        }
    }

    /// Whether `value`, followed by CRLF and the delimiter, would end early:
    /// it holds a delimiter itself, which a browser picks a boundary to avoid.
    fn holds_delimiter(value: &[u8], boundary: &[u8]) -> bool {
        let delimiter = [b"\r\n--", boundary].concat();
        let mut with_end = value.to_vec();
        with_end.extend_from_slice(b"\r\n");
        (0..with_end.len()).any(|i| {
            with_end[i..].starts_with(&delimiter)
                && matches!(
                    with_end.get(i + delimiter.len()),
                    None | Some(b'-' | b'\r' | b' ' | b'\t')
                )
        })
    }

    #[test]
    fn multipart_fields_and_files_read_back() {
        let mut rng = Rng::new(11);
        let mut checked = 0;
        for _ in 0..3000 {
            let boundary = rng.upto(20, b"abXY09'()+_,-./:=? ");
            let boundary = String::from_utf8(boundary).unwrap();
            let boundary = boundary.trim();
            if boundary.is_empty() {
                continue;
            }
            let mut body = Vec::new();
            if rng.one_in(3) {
                body.extend_from_slice(b"preamble\r\n");
            }
            let mut sent = Vec::new();
            for i in 0..rng.below(5) {
                let value = rng.upto(40, b"ab\r\n-\x00\xff ");
                let file = rng.one_in(2);
                sent.push((format!("f{i}"), value, file));
            }
            if sent
                .iter()
                .any(|(_, v, _)| holds_delimiter(v, boundary.as_bytes()))
            {
                continue;
            }
            for (name, value, file) in &sent {
                body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
                let disposition = format!("Content-Disposition: form-data; name=\"{name}\"");
                body.extend_from_slice(disposition.as_bytes());
                if *file {
                    body.extend_from_slice(b"; filename=\"a.bin\"\r\nContent-Type: x/y");
                }
                body.extend_from_slice(b"\r\n\r\n");
                body.extend_from_slice(value);
                body.extend_from_slice(b"\r\n");
            }
            body.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
            let ct = format!("multipart/form-data; boundary=\"{boundary}\"");
            let f = Form::new(Some(&ct), &body);
            for (name, value, file) in &sent {
                if *file {
                    let got = f.file(name).map(|f| f.bytes);
                    assert_eq!(got, Some(&value[..]), "{body:?}");
                } else {
                    let text = String::from_utf8_lossy(value);
                    assert_eq!(f.get(name), Some(text), "{body:?}");
                }
            }
            checked += 1;
            for _ in 0..4 {
                let mut broken = body.clone();
                mutate(&mut rng, &mut broken);
                let f = Form::new(Some(&ct), &broken);
                let _ = (f.iter().count(), f.files("f1").count());
            }
        }
        assert!(checked > 1000);
    }
}
