//! Where durable tables keep their rows: a [`Store`]. By default each
//! table is a log file in the data folder (`WISP_DATA`; `.wisp/data` in the
//! project in dev builds, `data` in release ones), and `wisp::store(...)` in
//! `init` puts a database of your own there instead.
//!
//! A log is a line per change, `ID\tJSON\n` (the row as it is now) or
//! `ID\t\n` (the row was removed), appended with one write each, so a
//! process that dies loses nothing it answered. Reading one back keeps each
//! row's last line. A line the crash cut short, the log's last, is dropped.
//! Once a log is more than twice the rows it holds (and past a megabyte), it
//! is written out again with only those rows, to a file of its own that
//! replaces it whole: a crash in between leaves the old log.
//!
//! `WISP_FSYNC` says when the log reaches the disk itself, which is what
//! survives the machine losing power: `second` (the default: at most a
//! second after a change), `always` (before the request is answered; a disk
//! flush per write), or `off` (when the operating system does it).

#[cfg(not(target_arch = "wasm32"))]
use crate::Error;
use crate::Result;
use std::sync::RwLock;
#[cfg(not(target_arch = "wasm32"))]
use std::sync::atomic::{AtomicBool, Ordering};

/// A place durable tables keep their rows in: a database, a key-value store.
/// Rows go in and out as the JSON of their value; row 0, when there is
/// one, holds the last id given (as text), so ids are never given twice.
/// Its calls come with the table's lock held, one at a time per table, in
/// the order of the changes.
///
/// ```ignore
/// // src/hooks.rs: every durable table in SQLite.
/// struct Db(std::sync::Mutex<rusqlite::Connection>);
///
/// impl wisp::Store for Db {
///     fn load(&self, table: &str) -> Result<Vec<(u64, String)>> {
///         let db = self.0.lock().unwrap();
///         db.execute(&format!("create table if not exists {table} (id integer primary key, json text)"), ())?;
///         let mut rows = db.prepare(&format!("select id, json from {table}"))?;
///         let rows = rows.query_map((), |r| Ok((r.get(0)?, r.get(1)?)))?;
///         Ok(rows.collect::<Result<_, _>>()?)
///     }
///     fn save(&self, table: &str, id: u64, json: Option<&str>) -> Result {
///         let db = self.0.lock().unwrap();
///         match json {
///             Some(j) => db.execute(&format!("insert or replace into {table} values (?1, ?2)"), (id, j))?,
///             None => db.execute(&format!("delete from {table} where id = ?1"), (id,))?,
///         };
///         Ok(())
///     }
/// }
///
/// fn init() -> Result {
///     wisp::store(Db(std::sync::Mutex::new(rusqlite::Connection::open("app.db")?)));
///     Ok(())
/// }
/// ```
pub trait Store: Send + Sync + 'static {
    /// Every row of `table`, as its id and JSON, in any order. Called once
    /// per table, before its first use.
    fn load(&self, table: &str) -> Result<Vec<(u64, String)>>;
    /// Keeps row `id` of `table`: `Some(json)` is its value now, `None`
    /// means it was removed. An error fails the request that made the
    /// change, and the table in memory stays as it was.
    fn save(&self, table: &str, id: u64, json: Option<&str>) -> Result;
}

/// Keeps every durable table (`#[derive(Rest)]` types, `Table::saved`) in
/// `store` from now on, in place of the log files. Call it in `init`, before
/// a table is used: a table reads its rows once, from where they are then.
pub fn store(store: impl Store) {
    let store: &'static dyn Store = Box::leak(Box::new(store));
    *CUSTOM.write().unwrap_or_else(|e| e.into_inner()) = Some(store);
}

static CUSTOM: RwLock<Option<&'static dyn Store>> = RwLock::new(None);

/// Set by the test client: tables stay in memory unless a store is given.
#[cfg(not(target_arch = "wasm32"))]
static MEMORY: AtomicBool = AtomicBool::new(false);

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn memory() {
    MEMORY.store(true, Ordering::Relaxed);
}

/// Where a durable table loads from and saves to: `None` keeps it in
/// memory (the edge build, tests, `WISP_DATA=off`).
pub(crate) fn current() -> Option<&'static dyn Store> {
    if let Some(s) = *CUSTOM.read().unwrap_or_else(|e| e.into_inner()) {
        return Some(s);
    }
    #[cfg(target_arch = "wasm32")]
    return None;
    #[cfg(not(target_arch = "wasm32"))]
    {
        if MEMORY.load(Ordering::Relaxed) {
            return None;
        }
        files::default().map(|f| f as &'static dyn Store)
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn invalid_name(table: &str) -> Option<Error> {
    let ok = !table.is_empty()
        && table.len() <= 64
        && table
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-');
    (!ok).then(|| {
        Error::new(
            500,
            format!("`{table}` cannot name a table: use letters, digits, `_` and `-`"),
        )
    })
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) mod files {
    use super::*;
    use std::collections::HashMap;
    use std::fs::{self, File, OpenOptions};
    use std::io::Write;
    use std::path::{Path, PathBuf};
    use std::sync::{Arc, Mutex, OnceLock};

    /// Past this size a log may be written out again.
    const COMPACT_AFTER: u64 = 1 << 20;

    #[derive(Clone, Copy, PartialEq, Eq, Debug)]
    pub(crate) enum Sync {
        Always,
        Second,
        Off,
    }

    /// The log files in one folder.
    pub(crate) struct Files {
        dir: PathBuf,
        sync: Sync,
        logs: Mutex<Vec<Log>>,
    }

    struct Log {
        table: String,
        path: PathBuf,
        file: Arc<File>,
        /// Bytes in the file.
        size: u64,
        /// The length of each row's last line, for the bytes the rows need.
        live: HashMap<u64, u64>,
        live_bytes: u64,
        /// Written since it last reached the disk.
        dirty: bool,
    }

    /// The store for `WISP_DATA`, made once; `None` for `off`.
    pub(crate) fn default() -> Option<&'static Files> {
        static FILES: OnceLock<Option<&'static Files>> = OnceLock::new();
        *FILES.get_or_init(|| {
            let dir = match crate::setting::<String>("WISP_DATA", "a folder") {
                Some(d) if d.eq_ignore_ascii_case("off") => return None,
                Some(d) => PathBuf::from(d),
                None if cfg!(debug_assertions) => {
                    let root = crate::sign::ROOT.get().copied().unwrap_or(".");
                    Path::new(root).join(".wisp").join("data")
                }
                None => PathBuf::from("data"),
            };
            let sync = match crate::setting::<String>("WISP_FSYNC", "always, second or off")
                .map(|s| s.to_ascii_lowercase())
                .as_deref()
            {
                None | Some("second") => Sync::Second,
                Some("always") => Sync::Always,
                Some("off") => Sync::Off,
                Some(v) => crate::fail(&format!(
                    "WISP_FSYNC is {v:?}, which is not always, second or off"
                )),
            };
            let files: &'static Files = Box::leak(Box::new(Files::new(dir, sync)));
            if sync == Sync::Second {
                let _ = std::thread::Builder::new()
                    .name("wisp-fsync".into())
                    .spawn(move || {
                        loop {
                            std::thread::sleep(std::time::Duration::from_secs(1));
                            files.sync_dirty();
                        }
                    });
            }
            Some(files)
        })
    }

    /// Makes sure what the logs hold is on the disk, when the server stops.
    /// A server that never loaded a durable table has nothing to flush.
    pub(crate) fn flush() {
        if USED.load(Ordering::Relaxed)
            && let Some(f) = default()
        {
            f.sync_dirty();
        }
    }

    static USED: AtomicBool = AtomicBool::new(false);

    impl Files {
        pub(crate) fn new(dir: PathBuf, sync: Sync) -> Files {
            Files {
                dir,
                sync,
                logs: Mutex::new(Vec::new()),
            }
        }

        fn logs(&self) -> std::sync::MutexGuard<'_, Vec<Log>> {
            self.logs.lock().unwrap_or_else(|e| e.into_inner())
        }

        /// Every log written since it last reached the disk, synced. The
        /// files are synced outside the lock, so writes go on meanwhile.
        pub(crate) fn sync_dirty(&self) {
            let dirty: Vec<Arc<File>> = self
                .logs()
                .iter_mut()
                .filter_map(|l| std::mem::take(&mut l.dirty).then(|| l.file.clone()))
                .collect();
            for f in dirty {
                if let Err(e) = f.sync_data() {
                    crate::http::log(format_args!("wisp: could not sync a table to disk: {e}"));
                }
            }
        }
    }

    fn io(table: &str, what: &str, e: std::io::Error) -> Error {
        Error::new(500, format!("table `{table}`: could not {what}: {e}"))
    }

    /// The rows of a log's text, each id's last line, and the length of
    /// the whole lines (a last line without its `\n` was cut short).
    fn replay(text: &[u8]) -> (HashMap<u64, &[u8]>, usize) {
        let whole = text.iter().rposition(|&b| b == b'\n').map_or(0, |i| i + 1);
        let mut rows = HashMap::new();
        for line in text[..whole].split(|&b| b == b'\n') {
            let Some(tab) = line.iter().position(|&b| b == b'\t') else {
                continue;
            };
            let Some(id) = std::str::from_utf8(&line[..tab])
                .ok()
                .and_then(|s| s.parse::<u64>().ok())
            else {
                continue;
            };
            let json = &line[tab + 1..];
            if json.is_empty() {
                rows.remove(&id);
            } else {
                rows.insert(id, json);
            }
        }
        (rows, whole)
    }

    impl Store for Files {
        fn load(&self, table: &str) -> Result<Vec<(u64, String)>> {
            if let Some(e) = invalid_name(table) {
                return Err(e);
            }
            USED.store(true, Ordering::Relaxed);
            fs::create_dir_all(&self.dir).map_err(|e| io(table, "make its folder", e))?;
            let path = self.dir.join(format!("{table}.log"));
            // A compaction the process did not finish: the log is still whole.
            let _ = fs::remove_file(path.with_extension("log.tmp"));
            let text = match fs::read(&path) {
                Ok(t) => t,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
                Err(e) => return Err(io(table, "read its log", e)),
            };
            let (rows, whole) = replay(&text);
            if whole < text.len() {
                crate::http::log(format_args!(
                    "wisp: table `{table}`: dropped the last {} bytes of its log, a write cut short",
                    text.len() - whole
                ));
                OpenOptions::new()
                    .write(true)
                    .open(&path)
                    .and_then(|f| f.set_len(whole as u64))
                    .map_err(|e| io(table, "repair its log", e))?;
            }
            let file = OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path)
                .map_err(|e| io(table, "open its log", e))?;
            let mut out = Vec::with_capacity(rows.len());
            let mut live = HashMap::with_capacity(rows.len());
            let mut live_bytes = 0;
            for (id, json) in rows {
                let json = String::from_utf8(json.to_vec()).map_err(|_| {
                    Error::new(500, format!("table `{table}`: row {id} is not UTF-8"))
                })?;
                let n = line_len(id, json.len());
                live.insert(id, n);
                live_bytes += n;
                out.push((id, json));
            }
            let mut logs = self.logs();
            logs.retain(|l| l.table != table);
            logs.push(Log {
                table: table.to_string(),
                path,
                file: Arc::new(file),
                size: whole as u64,
                live,
                live_bytes,
                dirty: false,
            });
            Ok(out)
        }

        fn save(&self, table: &str, id: u64, json: Option<&str>) -> Result {
            let mut logs = self.logs();
            let Some(log) = logs.iter_mut().find(|l| l.table == table) else {
                return Err(Error::new(
                    500,
                    format!("table `{table}` was saved before it was loaded"),
                ));
            };
            let body = json.unwrap_or("");
            let mut line = Vec::with_capacity(body.len() + 22);
            line.extend_from_slice(id.to_string().as_bytes());
            line.push(b'\t');
            line.extend_from_slice(body.as_bytes());
            line.push(b'\n');
            (&*log.file)
                .write_all(&line)
                .map_err(|e| io(table, "write its log", e))?;
            log.size += line.len() as u64;
            let n = if json.is_some() { line.len() as u64 } else { 0 };
            let old = match n {
                0 => log.live.remove(&id),
                n => log.live.insert(id, n),
            };
            log.live_bytes = log.live_bytes + n - old.unwrap_or(0);
            match self.sync {
                Sync::Always => log
                    .file
                    .sync_data()
                    .map_err(|e| io(table, "sync its log", e))?,
                Sync::Second => log.dirty = true,
                Sync::Off => {}
            }
            if log.size > COMPACT_AFTER && log.size > 2 * log.live_bytes {
                compact(log)?;
            }
            Ok(())
        }
    }

    fn line_len(id: u64, json: usize) -> u64 {
        (id.to_string().len() + json + 2) as u64
    }

    /// Writes the log out again with each row's last line only, then puts
    /// it in the old one's place.
    fn compact(log: &mut Log) -> Result {
        let table = log.table.clone();
        let text = fs::read(&log.path).map_err(|e| io(&table, "read its log", e))?;
        let (rows, _) = replay(&text);
        let mut ids: Vec<u64> = rows.keys().copied().collect();
        ids.sort_unstable();
        let mut out = Vec::with_capacity(log.live_bytes as usize);
        for id in ids {
            out.extend_from_slice(id.to_string().as_bytes());
            out.push(b'\t');
            out.extend_from_slice(rows[&id]);
            out.push(b'\n');
        }
        let tmp = log.path.with_extension("log.tmp");
        let write = || -> std::io::Result<File> {
            let mut f = File::create(&tmp)?;
            f.write_all(&out)?;
            f.sync_all()?;
            fs::rename(&tmp, &log.path)?;
            // The rename itself reaches the disk with the folder's entry.
            #[cfg(unix)]
            if let Some(dir) = log.path.parent() {
                File::open(dir)?.sync_all()?;
            }
            OpenOptions::new().append(true).open(&log.path)
        };
        let file = write().map_err(|e| io(&table, "compact its log", e))?;
        log.file = Arc::new(file);
        log.size = out.len() as u64;
        log.live_bytes = log.size;
        log.dirty = false;
        Ok(())
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        fn dir(name: &str) -> PathBuf {
            let d = std::env::temp_dir().join(format!("wisp-store-{name}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&d);
            d
        }

        #[test]
        fn logs_keep_the_last_line_of_each_row() {
            let d = dir("replay");
            let files = Files::new(d.clone(), Sync::Off);
            assert!(files.load("notes").unwrap().is_empty());
            files.save("notes", 1, Some("{\"a\":1}")).unwrap();
            files.save("notes", 2, Some("{\"a\":2}")).unwrap();
            files.save("notes", 1, Some("{\"a\":3}")).unwrap();
            files.save("notes", 2, None).unwrap();
            let again = Files::new(d.clone(), Sync::Always);
            assert_eq!(again.load("notes").unwrap(), [(1, "{\"a\":3}".to_string())]);
            assert!(files.load("../x").is_err(), "names stay in the folder");
            assert!(files.save("other", 1, None).is_err(), "loaded first");
            let _ = fs::remove_dir_all(d);
        }

        #[test]
        fn a_torn_last_line_is_dropped_and_the_log_repaired() {
            let d = dir("torn");
            fs::create_dir_all(&d).unwrap();
            fs::write(d.join("t.log"), "1\t\"a\"\n2\t\"b\"\n3\t\"c").unwrap();
            fs::write(d.join("t.log.tmp"), "junk").unwrap();
            let files = Files::new(d.clone(), Sync::Off);
            let mut rows = files.load("t").unwrap();
            rows.sort();
            assert_eq!(rows, [(1, "\"a\"".into()), (2, "\"b\"".into())]);
            assert!(!d.join("t.log.tmp").exists());
            files.save("t", 4, Some("\"d\"")).unwrap();
            assert_eq!(
                fs::read_to_string(d.join("t.log")).unwrap(),
                "1\t\"a\"\n2\t\"b\"\n4\t\"d\"\n"
            );
            let _ = fs::remove_dir_all(d);
        }

        #[test]
        fn big_logs_of_few_rows_are_compacted() {
            let d = dir("compact");
            let files = Files::new(d.clone(), Sync::Second);
            files.load("c").unwrap();
            let big = format!("\"{}\"", "x".repeat(1000));
            for i in 0..1500 {
                files.save("c", i % 3, Some(&big)).unwrap();
            }
            let size = fs::metadata(d.join("c.log")).unwrap().len();
            assert!(size < COMPACT_AFTER, "compacted: {size}");
            files.sync_dirty();
            let mut rows = Files::new(d.clone(), Sync::Off).load("c").unwrap();
            rows.sort();
            assert_eq!(rows.iter().map(|r| r.0).collect::<Vec<_>>(), [0, 1, 2]);
            assert!(rows.iter().all(|r| r.1 == big));
            let _ = fs::remove_dir_all(d);
        }
    }
}
