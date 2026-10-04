//! Blobs: files kept by their content, outside the rows that name them. An
//! [`Upload`] of any type goes in the blob store and a table keeps only its
//! hash, name and size, so a row stays small and a file is kept once however
//! many rows have it.
//!
//! ```ignore
//! #[model]
//! pub struct Doc { title: String, file: Upload }
//!
//! let file = cx.form().file("file").or_status(400)?;
//! DOCS.add(Doc { title, file: Upload::new(&file, "pdf csv")? });
//! // markup: <a href={doc.file}>{doc.file.name}</a>   (an Upload shows its URL)
//! ```
//!
//! Files are in `WISP_BLOBS` (a folder; by default `blobs` beside the data
//! folder, or in memory where tables are: tests, `WISP_DATA=off`). For S3 or
//! another place, `wisp::blobs(impl Blobs)` in `init`. A file is served at
//! `/_wisp/blob/<hash>` ([`serve`]) typed by its bytes (images) or as opaque
//! data, `nosniff`, and kept by browsers for good, as its hash never
//! names another file.

use crate::json::{FromJson, Problems};
use crate::{Error, Json, Response, Result, Shared, Value};
use std::collections::BTreeMap;
use std::fmt;
use std::sync::RwLock;

/// Where blob URLs start.
pub const PREFIX: &str = "/_wisp/blob/";

/// A place files are kept: a bucket, another disk. A file is its SHA-256 in
/// hex (64 characters) and its bytes.
///
/// `put` and `get` run on the thread that serves the request, and wait for
/// the place to answer: keep them to what a disk or a nearby cache does,
/// and have a slow store (a remote bucket) answer from a local copy.
pub trait Blobs: Send + Sync + 'static {
    /// Keeps `bytes` under `hash`, unless it has them already.
    fn put(&self, hash: &str, bytes: &[u8]) -> Result;
    /// The bytes of `hash`.
    fn get(&self, hash: &str) -> Option<Vec<u8>>;
}

static CUSTOM: RwLock<Option<&'static dyn Blobs>> = RwLock::new(None);

/// Keeps blobs in `blobs` from now on, in place of a folder. Call it in `init`.
pub fn blobs(blobs: impl Blobs) {
    let blobs: &'static dyn Blobs = Box::leak(Box::new(blobs));
    *CUSTOM.write().unwrap_or_else(|e| e.into_inner()) = Some(blobs);
}

/// Files in a folder, one per hash, written whole and then renamed into
/// place, so a crash leaves no half file under a hash.
struct Folder(std::path::PathBuf);

impl Blobs for Folder {
    fn put(&self, hash: &str, bytes: &[u8]) -> Result {
        let path = self.0.join(hash);
        if path.exists() {
            return Ok(());
        }
        let io = |e: std::io::Error| Error::new(500, format!("could not keep a file: {e}"));
        std::fs::create_dir_all(&self.0).map_err(io)?;
        // Two puts of one file may race: each has its own, the last rename wins, both whole.
        let tmp = self.0.join(format!(
            "{hash}.{}.tmp",
            crate::hex(&crate::sign::random::<4>())
        ));
        // On the disk before the rename: a crash must not leave an empty
        // file under a hash, which `put` would then take for the file.
        let kept = (|| {
            use std::io::Write;
            let mut f = std::fs::File::create(&tmp)?;
            f.write_all(bytes)?;
            f.sync_all()?;
            // Closed before the rename (wasm's `File` has nothing to drop).
            #[cfg(not(target_arch = "wasm32"))]
            drop(f);
            std::fs::rename(&tmp, &path)?;
            // The rename reaches the disk with the folder's entry.
            #[cfg(unix)]
            std::fs::File::open(&self.0)?.sync_all()?;
            Ok(())
        })();
        kept.map_err(|e| {
            let _ = std::fs::remove_file(&tmp);
            io(e)
        })
    }

    fn get(&self, hash: &str) -> Option<Vec<u8>> {
        std::fs::read(self.0.join(hash)).ok()
    }
}

/// Files in memory, for tests and apps without a data folder.
struct Memory(Shared<BTreeMap<String, Vec<u8>>>);

impl Blobs for Memory {
    fn put(&self, hash: &str, bytes: &[u8]) -> Result {
        self.0
            .lock()
            .entry(hash.into())
            .or_insert_with(|| bytes.to_vec());
        Ok(())
    }

    fn get(&self, hash: &str) -> Option<Vec<u8>> {
        self.0.lock().get(hash).cloned()
    }
}

static MEMORY: Memory = Memory(Shared::new(BTreeMap::new()));

/// Where blobs are now: the app's, `WISP_BLOBS`, the data folder's `blobs`
/// beside it, or memory where tables are.
fn current() -> &'static dyn Blobs {
    if let Some(b) = *CUSTOM.read().unwrap_or_else(|e| e.into_inner()) {
        return b;
    }
    static FOLDER: std::sync::OnceLock<Option<&'static Folder>> = std::sync::OnceLock::new();
    let folder = FOLDER.get_or_init(|| {
        let dir = match crate::setting::<String>("WISP_BLOBS", "a folder") {
            Some(d) => std::path::PathBuf::from(d),
            None if crate::store::current().is_none() => return None,
            None => match crate::setting::<String>("WISP_DATA", "a folder") {
                Some(d) => std::path::Path::new(&d).join("blobs"),
                None if cfg!(debug_assertions) => {
                    std::path::Path::new(crate::sign::ROOT.get().copied().unwrap_or("."))
                        .join(".wisp")
                        .join("blobs")
                }
                None => "blobs".into(),
            },
        };
        Some(Box::leak(Box::new(Folder(dir))))
    });
    match folder {
        Some(f) => *f,
        None => &MEMORY,
    }
}

/// Where `bytes` are kept: their SHA-256, hex, so the same bytes are kept
/// once.
fn hash(bytes: &[u8]) -> String {
    crate::hex(&wisp_shared::sha256::sha256(&[bytes]))
}

/// The bytes kept under `hash`; `None` when there are none, or `hash` is not
/// a hash (so it is safe to pass what a URL says).
pub fn get(hash: &str) -> Option<Vec<u8>> {
    let hash_like =
        hash.len() == 64 && hash.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'));
    hash_like.then(|| current().get(hash)).flatten()
}

/// The file at `/_wisp/blob/<hash>` as a response, cached for good; `None`
/// for any other path, or a file there is not.
pub(crate) fn serve(path: &str) -> Option<Response> {
    let hash = path.strip_prefix(PREFIX)?;
    let bytes = get(hash)?;
    let kind = crate::image::sniff(&bytes).unwrap_or("application/octet-stream");
    Some(
        Response::new(kind, bytes)
            .with_header("etag", format!("\"{hash}\""))
            .with_header("cache-control", "public, max-age=31536000, immutable")
            .with_header("x-content-type-options", "nosniff"),
    )
}

/// A file kept as a blob: what a row holds of it. In JSON an object with
/// `hash`, `name`, `type`, `size` and `url`; in markup its URL.
#[derive(Clone, Debug, PartialEq)]
pub struct Upload {
    /// Where the bytes are kept ([`get`]).
    pub hash: String,
    /// The name it had on the visitor's machine: visitor input, never a path.
    pub name: String,
    /// The type the browser sent: visitor input too.
    pub kind: String,
    pub size: usize,
}

impl Upload {
    /// Keeps a form's file. `types` are the extensions it may have, separated
    /// by spaces (`"pdf csv"`, any case); `""` takes any. Another is a 422 on
    /// `file`. A file of no bytes is refused too.
    pub fn new(file: &crate::File, types: &str) -> Result<Upload> {
        let upload = Upload::checked(file, types, "file")?;
        upload.store(file)?;
        Ok(upload)
    }

    /// [`Upload::new`] but not kept yet, a problem shown by the input
    /// `field`: an action's `Upload` is kept (`rt_traits::Keep`) only once all
    /// its inputs pass, so a refused form leaves no file behind.
    pub(crate) fn checked(file: &crate::File, types: &str, field: &str) -> Result<Upload> {
        let ext = file.name.rsplit_once('.').map_or("", |(_, e)| e);
        if !types.is_empty()
            && !types
                .split_whitespace()
                .any(|t| t.eq_ignore_ascii_case(ext))
        {
            let all: Vec<_> = types.split_whitespace().collect();
            return crate::invalid(field, format!("must be {}", all.join(", ")));
        }
        if file.bytes.is_empty() {
            return crate::invalid(field, "is empty");
        }
        Ok(Upload {
            hash: hash(file.bytes),
            name: file.name.to_string(),
            kind: file.content_type.to_string(),
            size: file.bytes.len(),
        })
    }

    /// Keeps `file`, the one this was [`Upload::checked`] from.
    pub(crate) fn store(&self, file: &crate::File) -> Result {
        current().put(&self.hash, file.bytes)
    }

    /// Where it is served: `/_wisp/blob/<hash>`.
    pub fn url(&self) -> String {
        format!("{PREFIX}{}", self.hash)
    }

    /// The file's bytes.
    pub fn bytes(&self) -> Option<Vec<u8>> {
        get(&self.hash)
    }
}

impl fmt::Display for Upload {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{PREFIX}{}", self.hash)
    }
}

impl Json for Upload {
    fn json(&self, out: &mut String) {
        out.push_str("{\"hash\":");
        self.hash.json(out);
        out.push_str(",\"name\":");
        self.name.json(out);
        out.push_str(",\"type\":");
        self.kind.json(out);
        out.push_str(",\"size\":");
        self.size.json(out);
        out.push_str(",\"url\":");
        self.url().json(out);
        out.push('}');
    }
}

impl FromJson for Upload {
    fn from_json(v: &Value, p: &mut Problems) -> Option<Upload> {
        let mut field = |name| match v.get(name) {
            Some(x) => Some(x),
            None => {
                p.check(name, Some("is required".into()));
                None
            }
        };
        let (hash, name, kind, size) = (
            field("hash")?,
            field("name")?,
            field("type")?,
            field("size")?,
        );
        Some(Upload {
            hash: p.read("hash", hash)?,
            name: p.read("name", name)?,
            kind: p.read("type", kind)?,
            size: p.read("size", size)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file<'a>(name: &'a str, bytes: &'a [u8]) -> crate::File<'a> {
        crate::File {
            name: name.into(),
            content_type: "application/pdf",
            bytes,
        }
    }

    #[test]
    fn kept_by_content_and_served() {
        crate::store::memory();
        let a = Upload::new(&file("a.PDF", b"%PDF-1"), "pdf csv").unwrap();
        let b = Upload::new(&file("b.csv", b"%PDF-1"), "pdf csv").unwrap();
        assert_eq!(a.hash, b.hash, "one copy");
        assert_eq!((a.size, a.name.as_str()), (6, "a.PDF"));
        assert_eq!(a.bytes().as_deref(), Some(&b"%PDF-1"[..]));
        let no = Upload::new(&file("a.exe", b"x"), "pdf csv").unwrap_err();
        assert_eq!((no.status(), no.message()), (422, "file: must be pdf, csv"));
        assert!(Upload::new(&file("a.pdf", b""), "").is_err());
        assert!(Upload::new(&file("a.exe", b"x"), "").is_ok());
        assert!(Upload::new(&file("noext", b"x"), "pdf").is_err());

        let r = serve(&a.url()).unwrap();
        assert_eq!(r.content_type, "application/octet-stream");
        assert!(
            r.headers
                .iter()
                .any(|(n, v)| n == "cache-control" && v.contains("immutable"))
        );
        assert!(serve("/_wisp/blob/../x").is_none());
        assert!(serve("/other").is_none());
        assert!(get(&"0".repeat(64)).is_none());
        assert!(get("../etc/passwd").is_none());

        let json = crate::to_json(&a);
        assert!(json.contains(&format!("\"url\":\"{}\"", a.url())));
        assert_eq!(crate::from_json::<Upload>(json.as_bytes()).unwrap(), a);
        assert_eq!(a.to_string(), a.url());
    }

    #[test]
    fn a_folder_keeps_files_whole() {
        let dir = std::env::temp_dir().join(format!("wisp-blobs-{}", std::process::id()));
        let f = Folder(dir.clone());
        let hash = "a".repeat(64);
        f.put(&hash, b"one").unwrap();
        f.put(&hash, b"two").unwrap();
        assert_eq!(
            f.get(&hash).as_deref(),
            Some(&b"one"[..]),
            "kept, not replaced"
        );
        assert_eq!(
            std::fs::read_dir(&dir).unwrap().count(),
            1,
            "no temp file left"
        );
        assert!(f.get(&"b".repeat(64)).is_none());
        let _ = std::fs::remove_dir_all(dir);
    }
}
