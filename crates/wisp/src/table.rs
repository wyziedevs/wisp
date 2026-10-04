//! `Table<T>`: rows under ids Wisp gives them, for state a page or an API
//! shares across requests: `static TODOS: Table<String> = Table::new();`,
//! then `TODOS.add(text)`, `TODOS.remove(id)`, `{#each TODOS.all() as todo}`.
//!
//! `Table::new()` keeps them in memory. `Table::saved("todos")` also keeps
//! them in the app's [`Store`](crate::Store) (log files in `WISP_DATA`, by
//! default), so they are there again after a restart: it reads them back
//! on first use, and saves each change before it is made in memory.
//! `#[derive(Rest)]` types have a saved table of their own (`Note::table()`),
//! which a `+server.rs` serves as a JSON API (see `rest.rs`).

use crate::json::FromJson;
use crate::store::{self, Store};
use crate::{Json, Result, Shared, Value};
use std::collections::BTreeMap;
use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{RwLock, RwLockReadGuard, RwLockWriteGuard};

/// Rows under ids from 1 up (or random ones), kept in order of their ids.
/// Reads share the lock; a change has it alone. After a change that
/// panicked, a saved table reads its rows again from its store.
pub struct Table<T> {
    rows: RwLock<Rows<T>>,
    /// Where a saved table keeps its rows; `None` keeps them in memory.
    saved: Option<Saved<T>>,
    /// Ids are random numbers under 2^53, not counted up.
    random: bool,
    /// A field no two rows share: its name and how to read it ([`Table::unique`]).
    unique: Option<Unique<T>>,
    /// Changes each stored row's JSON as it is read ([`Table::migrate`]).
    migrate: Option<fn(&mut Value)>,
    /// Hashes a row's `Password`s as it enters: `None` for a row type with
    /// none, which pays nothing.
    seal: Option<fn(&mut T, bool)>,
    /// Each change is sent on the channel named as the table ([`Table::live`]).
    live: bool,
    /// Where the store's changes were read up to ([`Store::changes`]).
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    // the edge build has no thread to poll
    cursor: AtomicU64,
    /// Changes saved from here, so a poll can tell the table was written
    /// while it asked the store.
    writes: AtomicU64,
}

/// A unique field's name and how to read it.
type Unique<T> = (&'static str, fn(&T) -> &str);

/// A saved table's name in the store, and how its rows become JSON and back.
struct Saved<T> {
    name: &'static str,
    write: fn(&T, &mut String),
    read: fn(&[u8]) -> Result<T>,
}

pub(crate) struct Rows<T> {
    pub(crate) last: u64,
    pub(crate) map: BTreeMap<u64, T>,
    /// A unique field's values, by the id of the row that has each.
    index: BTreeMap<String, u64>,
    state: State,
}

#[derive(Clone, Copy)]
enum State {
    /// A saved table whose rows are not read yet: from this store, or the
    /// app's (`None`) on first use.
    Unread(Option<&'static dyn Store>),
    /// Kept in memory only.
    Memory,
    /// Read from the store, which keeps each change.
    Stored(&'static dyn Store),
}

impl<T> Rows<T> {
    /// Rows that may be ahead of `store`, or half changed: read again from
    /// it on next use.
    fn stale(&mut self, store: &'static dyn Store) {
        self.map.clear();
        self.index.clear();
        self.last = 0;
        self.state = State::Unread(Some(store));
    }
}

/// A row and its id. It reads as its value (`row.title`, `{row}`), and as
/// JSON it is the value's object with `"id"` first.
#[derive(Debug, Clone, PartialEq)]
pub struct Row<T> {
    pub id: u64,
    pub value: T,
}

impl<T> std::ops::Deref for Row<T> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.value
    }
}

impl<T: fmt::Display> fmt::Display for Row<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.value.fmt(f)
    }
}

impl<T: Json> Json for Row<T> {
    fn json(&self, out: &mut String) {
        row_json(out, self.id, &self.value);
    }
}

impl<T: FromJson> FromJson for Row<T> {
    fn from_json(v: &crate::Value, p: &mut crate::json::Problems) -> Option<Row<T>> {
        let id = match v.get("id") {
            Some(id) => p.read("id", id)?,
            None => {
                p.check("id", Some("is required".into()));
                return None;
            }
        };
        // `{"id":1,"value":â€¦}`, for a value that is not an object.
        let value = match v.get("value") {
            Some(x) if matches!(v, crate::Value::Object(m) if m.len() == 2) => p.read("value", x),
            _ => T::from_json(v, p),
        }?;
        Some(Row { id, value })
    }
}

/// [`row_json`] of a value already written as `json`.
pub(crate) fn splice(out: &mut String, id: u64, json: &str) {
    out.push_str("{\"id\":");
    id.json(out);
    match json.strip_prefix('{') {
        Some(rest) if rest.starts_with('}') => out.push('}'),
        Some(rest) => {
            out.push(',');
            out.push_str(rest);
        }
        None => {
            out.push_str(",\"value\":");
            out.push_str(json);
            out.push('}');
        }
    }
}

/// `{"id":1,â€¦the value's members}`; a value that is not an object is
/// `{"id":1,"value":â€¦}`.
pub(crate) fn row_json<T: Json + ?Sized>(out: &mut String, id: u64, value: &T) {
    out.push_str("{\"id\":");
    id.json(out);
    let at = out.len();
    value.json(out);
    if out[at..].starts_with("{}") {
        out.truncate(at);
        out.push('}');
    } else if out[at..].starts_with('{') {
        out.replace_range(at..at + 1, ",");
    } else {
        out.insert_str(at, ",\"value\":");
        out.push('}');
    }
}

/// Names of the saved tables loaded so far, with the table each belongs to:
/// two tables under one name would write over each other's rows.
static NAMES: Shared<Vec<(&'static str, usize)>> = Shared::new(Vec::new());

/// A table the poll thread follows.
#[cfg_attr(target_arch = "wasm32", allow(dead_code))] // the edge build has no thread to poll
trait Poll: Sync {
    fn poll(&self) -> Result;
}

impl<T: Send + Sync> Poll for Table<T> {
    fn poll(&self) -> Result {
        Table::poll(self)
    }
}

/// The names of the `.live()` tables `ready` has seen: what `/_wisp/live/NAME`
/// serves, which the pages reading them listen to.
static LIVE: Shared<Vec<&'static str>> = Shared::new(Vec::new());

/// The event stream of the live table `name`: a message for each change.
/// `None` for any other name, so no other channel can be listened to.
pub(crate) fn live_events(name: &str) -> Option<crate::Response> {
    // The edge build has no channels.
    #[cfg(target_arch = "wasm32")]
    return {
        let _ = name;
        None
    };
    #[cfg(not(target_arch = "wasm32"))]
    {
        let found = LIVE.lock().iter().copied().find(|n| *n == name)?;
        Some(crate::channel(found).events())
    }
}

/// Every table `ready` has seen, for `test::fresh`.
static EVERY: Shared<Vec<&'static dyn Wipe>> = Shared::new(Vec::new());

trait Wipe: Sync {
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    fn wipe(&self);
}

impl<T: Send + Sync> Wipe for Table<T> {
    fn wipe(&self) {
        self.clear();
    }
}

/// Empties every table, for a test that starts from nothing.
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
pub(crate) fn wipe_all() {
    for t in EVERY.lock().iter() {
        t.wipe();
    }
}

/// Tables that follow their store (`WISP_STORE_POLL`).
static POLLED: Shared<Vec<&'static dyn Poll>> = Shared::new(Vec::new());

/// Whether `WISP_STORE_POLL` is set; the first time, starts the thread that
/// polls every `ready` table that long apart. A table whose store has no
/// `changes` is asked each time and never answers: nothing happens.
fn polling() -> bool {
    let secs: u64 = crate::env_or("WISP_STORE_POLL", 0);
    #[cfg(not(target_arch = "wasm32"))]
    if secs > 0 {
        static STARTED: std::sync::Once = std::sync::Once::new();
        STARTED.call_once(|| {
            let every = std::time::Duration::from_secs(secs);
            let _ = std::thread::Builder::new()
                .name("wisp-store-poll".into())
                .spawn(move || {
                    loop {
                        std::thread::sleep(every);
                        let all: Vec<_> = POLLED.lock().clone();
                        for t in all {
                            let r =
                                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| t.poll()));
                            if let Ok(Err(e)) = r {
                                crate::http::log(format_args!("wisp: store poll: {}", e.detail()));
                            }
                        }
                    }
                });
        });
    }
    secs > 0
}

impl<T> Table<T> {
    /// A table kept in memory: its rows are gone when the server stops.
    pub const fn new() -> Table<T> {
        Table::make(None, false)
    }

    /// No two rows may have the same `key` (a field, such as an email):
    /// `Table::saved("users").unique("email", |u: &User| &u.email)`. Looking one up
    /// by it is `by`, and a second one is refused: `try_add` and `try_update`
    /// give a 422 on `field` (`add`, `update` and `set` panic, a 500). Rows
    /// already saved with a repeat keep the last.
    pub const fn unique(mut self, field: &'static str, key: fn(&T) -> &str) -> Table<T> {
        self.unique = Some((field, key));
        self
    }

    /// Changes each saved row's JSON before it is read back, to bring old
    /// rows to the type as it is now: a renamed field, a new one with a
    /// default. Runs at load only: `.migrate(|row| rename(row, "name",
    /// "title"))`.
    pub const fn migrate(mut self, f: fn(&mut Value)) -> Table<T> {
        self.migrate = Some(f);
        self
    }

    /// Sends `change` on the channel named as the table (`wisp::channel(name)`)
    /// after every change, from any request or, with `WISP_STORE_POLL`, from
    /// another instance: a page listening to it can refresh. Nothing is sent
    /// while no one listens. A table in memory has no name: nothing is sent.
    pub const fn live(mut self) -> Table<T> {
        self.live = true;
        self
    }

    const fn make(saved: Option<Saved<T>>, random: bool) -> Table<T> {
        let state = match saved {
            Some(_) => State::Unread(None),
            None => State::Memory,
        };
        Table {
            rows: RwLock::new(Rows {
                last: 0,
                map: BTreeMap::new(),
                index: BTreeMap::new(),
                state,
            }),
            saved,
            random,
            unique: None,
            migrate: None,
            seal: None,
            live: false,
            cursor: AtomicU64::new(0),
            writes: AtomicU64::new(0),
        }
    }

    /// A table whose rows survive restarts, kept in the app's store under
    /// `name` (letters, digits, `_`, `-`): `static TODOS: Table<Todo> =
    /// Table::saved("todos");`. The row type must be `Json` and `FromJson`.
    pub const fn saved(name: &'static str) -> Table<T>
    where
        T: Json + FromJson,
    {
        Table::rest(Some(name), false)
    }

    /// For `#[derive(Rest)]`: saved under `name` unless it is `None`, with
    /// random ids if `random`.
    #[doc(hidden)]
    pub const fn rest(name: Option<&'static str>, random: bool) -> Table<T>
    where
        T: Json + FromJson,
    {
        let saved = match name {
            Some(name) => Some(Saved {
                name,
                write: crate::password::stored::<T>,
                read: crate::password::read::<T>,
            }),
            None => None,
        };
        let mut table = Table::make(saved, random);
        // `#[unique]` on a field of the row type.
        table.unique = T::UNIQUE;
        table.seal = if T::SEALS { Some(T::seal) } else { None };
        table
    }

    /// The rows, locked for reading, read from the store the first time.
    pub(crate) fn read(&self) -> RwLockReadGuard<'_, Rows<T>> {
        loop {
            // Poisoned: `write` mends it first.
            if let Ok(rows) = self.rows.read()
                && !matches!(rows.state, State::Unread(_))
            {
                return rows;
            }
            drop(self.write());
        }
    }

    /// The rows, locked for a change, read from the store the first time.
    /// After a change that panicked (an `update` whose closure did), a
    /// saved table reads its rows again, as the store has them: the one
    /// in memory may be half changed, and never saved.
    pub(crate) fn write(&self) -> RwLockWriteGuard<'_, Rows<T>> {
        let mut rows = self.rows.write().unwrap_or_else(|e| {
            self.rows.clear_poison();
            let mut rows = e.into_inner();
            if let State::Stored(store) = rows.state {
                rows.stale(store);
            } else {
                self.reindex(&mut rows);
            }
            rows
        });
        if let State::Unread(from) = rows.state {
            // A store that fails here panics, poisoned again.
            self.load(&mut rows, from.or_else(store::current));
        }
        rows
    }

    /// Reads a saved table's rows from the store now, rather than on first
    /// use: at startup, after `init` has set the store. A store that fails
    /// then stops the server before it answers anything. With
    /// `WISP_STORE_POLL` (seconds), the table then follows the store's
    /// changes, for a store other instances write to ([`Store::changes`]).
    #[doc(hidden)]
    pub fn ready(&'static self)
    where
        T: Send + Sync + 'static,
    {
        drop(self.write());
        let mut every = EVERY.lock();
        // Once: a second `ready` (each test client makes one) would have the
        // table polled twice.
        if every.iter().any(|t| std::ptr::addr_eq(*t, self)) {
            return;
        }
        every.push(self);
        drop(every);
        if let (true, Some(saved)) = (self.live, &self.saved) {
            LIVE.lock().push(saved.name);
        }
        if self.saved.is_none() || !matches!(self.read().state, State::Stored(_)) {
            return;
        }
        if polling() {
            POLLED.lock().push(self);
        }
    }

    /// The unique field's index, made again from the rows.
    fn reindex(&self, rows: &mut Rows<T>) {
        rows.index.clear();
        if let Some((_, key)) = self.unique {
            for (&id, v) in &rows.map {
                rows.index.insert(key(v).to_owned(), id);
            }
        }
    }

    /// The 422 for a value whose unique field another row (not `except`)
    /// has: `Ok` when it is free, or the table has none.
    fn free(&self, rows: &Rows<T>, value: &T, except: u64) -> Result {
        match self.unique {
            Some((field, key)) if rows.index.get(key(value)).is_some_and(|&id| id != except) => {
                crate::invalid(field, "is taken")
            }
            _ => Ok(()),
        }
    }

    /// Sends `change` to those listening, if the table is live. Not with
    /// the table locked: the relay's publish is the app's code, and may wait.
    pub(crate) fn notify(&self) {
        // Channels are not in the edge build.
        #[cfg(not(target_arch = "wasm32"))]
        if let (true, Some(saved)) = (self.live, &self.saved) {
            let c = crate::channel(saved.name);
            if c.subscribers() > 0 {
                c.send("change");
            }
        }
    }

    /// A stored row's value: its JSON, migrated if the table says how.
    fn read_row(&self, saved: &Saved<T>, json: &str) -> Result<T> {
        let Some(migrate) = self.migrate else {
            return (saved.read)(json.as_bytes());
        };
        let mut v = crate::json::parse(json).map_err(|e| crate::Error::new(500, e))?;
        migrate(&mut v);
        (saved.read)(crate::json::to_json(&v).as_bytes())
    }

    /// Reads a saved table's rows from `store`, which keeps its changes
    /// from then on; without one, the table is kept in memory.
    fn load(&self, rows: &mut Rows<T>, store: Option<&'static dyn Store>) {
        let (Some(saved), Some(store)) = (&self.saved, store) else {
            rows.state = State::Memory;
            return;
        };
        let name = saved.name;
        let me = self as *const Table<T> as usize;
        let mut names = NAMES.lock();
        match names.iter().find(|(n, _)| *n == name) {
            Some((_, t)) if *t != me => panic!(
                "two tables are named `{name}`, and would save over each other: \
                 name one with #[rest(table = \"...\")] or Table::saved(\"...\")"
            ),
            Some(_) => {}
            None => names.push((name, me)),
        }
        drop(names);
        let kept = store
            .load(name)
            .unwrap_or_else(|e| panic!("could not read table `{name}`: {}", e.detail()));
        for (id, json) in kept {
            if id == 0 {
                // The last id given, kept when the row that had it was
                // removed, so ids are not given twice.
                rows.last = rows.last.max(json.parse().unwrap_or(0));
                continue;
            }
            let value = self.read_row(saved, &json).unwrap_or_else(|e| {
                panic!(
                    "table `{name}`: row {id} is not a {} any more ({}). A field added \
                     since it was saved must be an `Option` (or a `bool`), so old rows read.",
                    std::any::type_name::<T>(),
                    e.message()
                )
            });
            rows.last = rows.last.max(id);
            rows.map.insert(id, value);
        }
        self.reindex(rows);
        rows.state = State::Stored(store);
    }

    /// A new row's id.
    pub(crate) fn next_id(&self, rows: &mut Rows<T>) -> u64 {
        if !self.random {
            rows.last += 1;
            return rows.last;
        }
        loop {
            let id = u64::from_le_bytes(crate::sign::random::<8>()) >> 11;
            if id > 0 && !rows.map.contains_key(&id) {
                return id;
            }
        }
    }

    /// Hashes the `Password`s `value` holds that are text typed, once, as it
    /// enters the table (not under its lock).
    fn seal(&self, value: &mut T) {
        if let Some(seal) = self.seal {
            seal(value, true);
        }
    }

    /// The JSON a saved table keeps for `value`; empty for one in memory.
    fn encode(&self, rows: &Rows<T>, value: &T) -> String {
        let mut json = String::new();
        if let (State::Stored(_), Some(saved)) = (rows.state, &self.saved) {
            (saved.write)(value, &mut json);
        }
        json
    }

    /// Saves row `id` of a saved table: `json` is its value's JSON, `None`
    /// when it is removed. See [`Table::keep`].
    pub(crate) fn save(&self, rows: &mut Rows<T>, id: u64, json: Option<&str>) -> Result {
        self.keep(rows, |store, name| store.save(name, id, json))
    }

    /// Saves rows just put in the table, as `(id, json)`, all or none. See
    /// [`Table::keep`].
    pub(crate) fn save_many(&self, rows: &mut Rows<T>, new: &[(u64, String)]) -> Result {
        self.keep(rows, |store, name| store.save_many(name, new))
    }

    /// Has a saved table's store keep a change. A store that fails is the
    /// request's 500, and the table is read again from the store on next
    /// use, not here under the lock: what is in memory may be ahead of it.
    fn keep(&self, rows: &mut Rows<T>, save: impl FnOnce(&dyn Store, &str) -> Result) -> Result {
        let (State::Stored(store), Some(saved)) = (rows.state, &self.saved) else {
            return Ok(());
        };
        self.writes.fetch_add(1, Ordering::Relaxed);
        save(store, saved.name).map_err(|e| {
            rows.stale(store);
            let why = format!("could not save table `{}`: {}", saved.name, e.detail());
            crate::Error::new(500, why)
        })
    }

    /// Reads a saved table's rows again from its store, for rows another
    /// instance of the app changes there: a stored row the one in memory
    /// lacks, or one `newer(in memory, stored)` says is newer, replaces it.
    /// For a table whose rows only grow newer and are never removed: the
    /// store is read unlocked, so a save meanwhile leaves memory ahead of
    /// what was read, which then changes nothing. The write lock is taken
    /// only for a row that does change, and briefly.
    pub(crate) fn refresh(&self, newer: fn(&T, &T) -> bool) -> Result {
        let unread = self
            .rows
            .read()
            .is_ok_and(|r| matches!(r.state, State::Unread(_)));
        if unread {
            // The first read is the whole table.
            drop(self.write());
            return Ok(());
        }
        let (State::Stored(store), Some(saved)) = (self.read().state, &self.saved) else {
            return Ok(());
        };
        let kept = store.load(saved.name)?;
        let mut fresh = Vec::with_capacity(kept.len());
        for (id, json) in kept.into_iter().filter(|(id, _)| *id != 0) {
            fresh.push((id, self.read_row(saved, &json)?));
        }
        let rows = self.read();
        fresh.retain(|(id, v)| rows.map.get(id).is_none_or(|old| newer(old, v)));
        drop(rows);
        if fresh.is_empty() {
            return Ok(());
        }
        let mut rows = self.write();
        for (id, v) in fresh {
            rows.last = rows.last.max(id);
            match rows.map.get_mut(&id) {
                Some(old) if newer(old, &v) => *old = v,
                Some(_) => {}
                None => {
                    rows.map.insert(id, v);
                }
            }
        }
        self.reindex(&mut rows);
        drop(rows);
        self.notify();
        Ok(())
    }

    /// Applies what the store's `changes` say happened since the last
    /// poll. The table is locked from before the store is asked, so a
    /// change made here meanwhile is not written over with an older row.
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))] // the edge build has no thread to poll
    fn poll(&self) -> Result {
        let (State::Stored(store), Some(saved)) = (self.read().state, &self.saved) else {
            return Ok(());
        };
        // Asked with the table unlocked: the store's round trip does not
        // stop the requests that read or write it.
        let (seen, cursor) = (
            self.writes.load(Ordering::Relaxed),
            self.cursor.load(Ordering::Relaxed),
        );
        let Some(mut changes) = store.changes(saved.name, cursor)? else {
            return Ok(());
        };
        if changes.rows.is_empty() {
            self.cursor.store(changes.cursor, Ordering::Relaxed);
            return Ok(());
        }
        let mut rows = self.write();
        // A change made here since would be written over with an older row:
        // asked again, now that nothing can be.
        if self.writes.load(Ordering::Relaxed) != seen {
            match store.changes(saved.name, cursor)? {
                Some(again) => changes = again,
                None => return Ok(()),
            }
        }
        // Every row is read before any is applied: one that does not read
        // (another instance's newer schema) leaves the table as it was, and
        // is tried again on the next poll, not half applied.
        let mut read = Vec::with_capacity(changes.rows.len());
        for (id, json) in changes.rows {
            // Row 0 is the last id given.
            let value = match (id, json) {
                (0, last) => {
                    rows.last = rows
                        .last
                        .max(last.and_then(|l| l.parse().ok()).unwrap_or(0));
                    continue;
                }
                (_, json) => json.map(|j| self.read_row(saved, &j)).transpose()?,
            };
            read.push((id, value));
        }
        for (id, value) in read {
            match value {
                None => {
                    rows.map.remove(&id);
                }
                Some(v) => {
                    rows.last = rows.last.max(id);
                    rows.map.insert(id, v);
                }
            }
        }
        self.reindex(&mut rows);
        self.cursor.store(changes.cursor, Ordering::Relaxed);
        drop(rows);
        self.notify();
        Ok(())
    }

    /// Panics with a store's error, which is the request's 500: for the
    /// calls that answer with a value, not a `Result`. `rows` is unlocked
    /// first, so the lock is not poisoned (`keep` had the table read
    /// again on next use).
    fn failed(rows: RwLockWriteGuard<'_, Rows<T>>, e: crate::Error) -> ! {
        drop(rows);
        panic!("{}", e.message())
    }

    /// Reads a saved table's rows from `store` now, for a test.
    #[cfg(test)]
    pub(crate) fn load_from(&self, store: &'static dyn Store) {
        self.load(&mut self.rows.write().unwrap(), Some(store));
    }

    /// Keeps `value` under a new id, which it returns. A table with a
    /// [`unique`](Table::unique) field panics (a 500) for a repeat: see
    /// [`Table::try_add`].
    pub fn add(&self, mut value: T) -> u64 {
        self.seal(&mut value);
        let mut rows = self.write();
        match self.insert(&mut rows, value) {
            Ok(id) => {
                drop(rows);
                self.notify();
                id
            }
            Err(e) => Self::failed(rows, e),
        }
    }

    /// Like [`Table::add`], but a value whose unique field another row has
    /// is a 422 on that field (`is taken`), for an action to return.
    pub fn try_add(&self, mut value: T) -> Result<u64> {
        self.seal(&mut value);
        let mut rows = self.write();
        let id = self.insert(&mut rows, value)?;
        drop(rows);
        self.notify();
        Ok(id)
    }

    fn insert(&self, rows: &mut Rows<T>, value: T) -> Result<u64> {
        self.free(rows, &value, 0)?;
        let id = self.next_id(rows);
        let json = self.encode(rows, &value);
        self.save(rows, id, Some(&json))?;
        if let Some((_, key)) = self.unique {
            rows.index.insert(key(&value).to_owned(), id);
        }
        rows.map.insert(id, value);
        Ok(id)
    }

    /// Saves that row `id` is gone and takes it out. When it had the last
    /// id given, the store keeps that id as row 0, so it is not given again
    /// after a restart.
    pub(crate) fn delete(&self, rows: &mut Rows<T>, id: u64) -> Result<Option<T>> {
        if !rows.map.contains_key(&id) {
            return Ok(None);
        }
        self.save(rows, id, None)?;
        if id == rows.last && !self.random {
            self.save(rows, 0, Some(&id.to_string()))?;
        }
        let gone = rows.map.remove(&id);
        if let (Some((_, key)), Some(v)) = (self.unique, &gone) {
            rows.index.remove(key(v));
        }
        Ok(gone)
    }

    /// Takes the row out; `None` when there is none.
    pub fn remove(&self, id: u64) -> Option<T> {
        let mut rows = self.write();
        match self.delete(&mut rows, id) {
            Ok(v) => {
                drop(rows);
                self.notify();
                v
            }
            Err(e) => Self::failed(rows, e),
        }
    }

    /// Changes the row in place: `NOTES.update(id, |n| n.done = true)`.
    /// `None` when there is none.
    pub fn update<R>(&self, id: u64, f: impl FnOnce(&mut T) -> R) -> Option<R> {
        let mut rows = self.write();
        let row = rows.map.get_mut(&id)?;
        let before = self.unique.map(|(_, key)| key(row).to_owned());
        let r = f(row);
        // A password set here is hashed with the table locked: hash it
        // first (`Password::new`) to keep the wait off it.
        self.seal(row);
        if let (Some((field, key)), Some(before)) = (self.unique, before) {
            let now = key(&rows.map[&id]);
            if now != before {
                if rows.index.get(now).is_some_and(|&o| o != id) {
                    // Half changed, as after a closure that panicked.
                    panic!("{field} is taken: change a unique field with try_update");
                }
                let now = now.to_owned();
                rows.index.remove(&before);
                rows.index.insert(now, id);
            }
        }
        let json = self.encode(&rows, &rows.map[&id]);
        if let Err(e) = self.save(&mut rows, id, Some(&json)) {
            Self::failed(rows, e);
        }
        drop(rows);
        self.notify();
        Some(r)
    }

    /// Replaces the row with `value`; `None` when there is none. A repeat
    /// of another row's unique field panics: see [`Table::try_set`].
    pub fn set(&self, id: u64, value: T) -> Option<()> {
        self.try_set(id, value)
            .unwrap_or_else(|e| panic!("{}", e.message()))
    }

    /// Like [`Table::set`], but a repeat of another row's unique field is
    /// a 422 on that field.
    pub fn try_set(&self, id: u64, mut value: T) -> Result<Option<()>> {
        self.seal(&mut value);
        let mut rows = self.write();
        if !rows.map.contains_key(&id) {
            return Ok(None);
        }
        self.replace(&mut rows, id, value)?;
        drop(rows);
        self.notify();
        Ok(Some(()))
    }

    /// Puts `value` in place of row `id`, which is there, saved first.
    fn replace(&self, rows: &mut Rows<T>, id: u64, value: T) -> Result {
        self.free(rows, &value, id)?;
        let json = self.encode(rows, &value);
        self.save(rows, id, Some(&json))?;
        if let Some((_, key)) = self.unique {
            let old = key(&rows.map[&id]).to_owned();
            rows.index.remove(&old);
            rows.index.insert(key(&value).to_owned(), id);
        }
        rows.map.insert(id, value);
        Ok(())
    }

    /// Takes every row out. Ids are not given again, as after `remove`.
    pub fn clear(&self) {
        let mut rows = self.write();
        let ids: Vec<u64> = rows.map.keys().copied().collect();
        for id in ids {
            if let Err(e) = self.delete(&mut rows, id) {
                Self::failed(rows, e);
            }
        }
        drop(rows);
        self.notify();
    }

    /// Reads the row where it is, without a copy: `NOTES.with(id, |n|
    /// n.title.len())`. `f` runs with the table locked, so it must not
    /// change the table.
    pub fn with<R>(&self, id: u64, f: impl FnOnce(&T) -> R) -> Option<R> {
        self.read().map.get(&id).map(f)
    }

    /// Calls `f` with each row's id and value, in order of their ids,
    /// without copies. `f` runs with the table locked, so it must not
    /// change the table.
    pub fn each(&self, mut f: impl FnMut(u64, &T)) {
        for (&id, v) in &self.read().map {
            f(id, v);
        }
    }

    pub fn len(&self) -> usize {
        self.read().map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.read().map.is_empty()
    }
}

impl<T: Clone> Table<T> {
    /// A copy of the row.
    pub fn get(&self, id: u64) -> Option<Row<T>> {
        let value = self.with(id, T::clone)?;
        Some(Row { id, value })
    }

    /// Keeps `value` unless `taken` is true of a row already there, checked
    /// with the table locked: two at once cannot both be kept.
    pub fn add_unless(&self, taken: impl Fn(&T) -> bool, mut value: T) -> Option<Row<T>> {
        self.seal(&mut value);
        let mut rows = self.write();
        if rows.map.values().any(taken) {
            return None;
        }
        let copy = value.clone();
        match self.insert(&mut rows, value) {
            Ok(id) => {
                drop(rows);
                self.notify();
                Some(Row { id, value: copy })
            }
            Err(e) => Self::failed(rows, e),
        }
    }

    /// The row whose unique field is `key`, found without a scan; `None`
    /// when there is none, or the table has no [`unique`](Table::unique).
    pub fn by(&self, key: &str) -> Option<Row<T>> {
        let rows = self.read();
        let id = *rows.index.get(key)?;
        Some(Row {
            id,
            value: rows.map.get(&id)?.clone(),
        })
    }

    /// Like [`Table::update`], but on a copy, so a unique field changed to
    /// another row's is a 422 on that field and the row stays as it was.
    pub fn try_update<R>(&self, id: u64, f: impl FnOnce(&mut T) -> R) -> Result<Option<R>> {
        // Read, changed and put back with the table locked: two at once
        // (`n.count += 1`) must both count. Hashing a password set here
        // waits on the lock too, as in `update`.
        let mut rows = self.write();
        let Some(mut v) = rows.map.get(&id).cloned() else {
            return Ok(None);
        };
        let r = f(&mut v);
        self.seal(&mut v);
        self.replace(&mut rows, id, v)?;
        drop(rows);
        self.notify();
        Ok(Some(r))
    }

    /// A copy of every row, in order of their ids (oldest first, unless
    /// the ids are random).
    pub fn all(&self) -> Vec<Row<T>> {
        self.filter(|_| true)
    }

    /// A copy of the first row for which `f` is true.
    pub fn find(&self, f: impl Fn(&T) -> bool) -> Option<Row<T>> {
        let rows = self.read();
        let (&id, v) = rows.map.iter().find(|(_, v)| f(v))?;
        Some(Row {
            id,
            value: v.clone(),
        })
    }

    /// A copy of every row for which `f` is true: `NOTES.filter(|n| !n.done)`.
    pub fn filter(&self, f: impl Fn(&T) -> bool) -> Vec<Row<T>> {
        let rows = self.read();
        rows.map
            .iter()
            .filter(|(_, v)| f(v))
            .map(|(&id, v)| Row {
                id,
                value: v.clone(),
            })
            .collect()
    }

    /// The page of rows the request's `?page=N` asks for (the first when it
    /// asks for none), `per` a page, newest (highest id) first: `let posts =
    /// POSTS.page(cx, 10);`, then `{#each posts as post}` and
    /// `{#if let Some(href) = posts.next}<a {href}>Older</a>{/if}`. Only
    /// that page's rows are copied.
    pub fn page(&self, cx: &crate::Cx, per: usize) -> Page<T> {
        let per = per.max(1);
        let number = cx.query_or("page", 1usize).max(1);
        let skip = (number - 1).saturating_mul(per);
        let rows = self.read();
        let page = rows
            .map
            .iter()
            .rev()
            .skip(skip)
            .take(per)
            .map(|(&id, v)| Row {
                id,
                value: v.clone(),
            })
            .collect();
        let more = rows.map.len() > skip.saturating_add(per);
        drop(rows);
        Page {
            rows: page,
            number,
            prev: (number > 1).then(|| page_href(cx, number - 1)),
            next: more.then(|| page_href(cx, number + 1)),
        }
    }
}

/// One page of a table's rows, from [`Table::page`]. It reads as its rows
/// (`{#each posts as post}`, `posts.len()`), and has links to the pages
/// on either side: `None` at the ends.
#[derive(Debug, Clone, PartialEq)]
pub struct Page<T> {
    pub rows: Vec<Row<T>>,
    /// Which page it is, from 1.
    pub number: usize,
    /// `?page=N` of the page before (newer rows), keeping the rest of the
    /// query.
    pub prev: Option<String>,
    /// `?page=N` of the page after (older rows).
    pub next: Option<String>,
}

impl<T> std::ops::Deref for Page<T> {
    type Target = [Row<T>];
    fn deref(&self) -> &[Row<T>] {
        &self.rows
    }
}

impl<'a, T> IntoIterator for &'a Page<T> {
    type Item = &'a Row<T>;
    type IntoIter = std::slice::Iter<'a, Row<T>>;
    fn into_iter(self) -> Self::IntoIter {
        self.rows.iter()
    }
}

impl<T: Json> Json for Page<T> {
    fn json(&self, out: &mut String) {
        out.push_str("{\"rows\":");
        self.rows.json(out);
        out.push_str(",\"number\":");
        self.number.json(out);
        out.push_str(",\"prev\":");
        self.prev.json(out);
        out.push_str(",\"next\":");
        self.next.json(out);
        out.push('}');
    }
}

/// `?page=n` with the rest of the request's query as it was sent (but an
/// action's `/name`). Relative to the page, so no path is echoed back.
fn page_href(cx: &crate::Cx, n: usize) -> String {
    format!("?{}page={n}", cx.query_without(&["page"]))
}

impl<T> Default for Table<T> {
    fn default() -> Table<T> {
        Table::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Value;
    use crate::json;
    use std::sync::Mutex;

    struct Note {
        title: String,
        done: bool,
    }

    impl Json for Note {
        fn json(&self, out: &mut String) {
            out.push_str("{\"title\":");
            self.title.json(out);
            out.push_str(",\"done\":");
            self.done.json(out);
            out.push('}');
        }
    }

    #[test]
    fn rows_have_ids_and_read_as_json() {
        let t: Table<String> = Table::new();
        assert_eq!((t.add("a".into()), t.add("b".into())), (1, 2));
        assert_eq!(t.remove(1).as_deref(), Some("a"));
        assert_eq!(t.add("c".into()), 3, "ids are not reused");
        assert_eq!(t.update(3, |s| s.push('!')), Some(()));
        assert_eq!(t.update(9, |s| s.push('!')), None);
        let all = t.all();
        assert_eq!((all.len(), all[1].to_string()), (2, "c!".to_string()));
        assert_eq!(
            json::to_json(&all),
            r#"[{"id":2,"value":"b"},{"id":3,"value":"c!"}]"#
        );
        assert_eq!(t.find(|s| s == "b").map(|r| r.id), Some(2));
        assert_eq!(t.filter(|s| s.ends_with('!')).len(), 1);
        let mut ids = Vec::new();
        t.each(|id, _| ids.push(id));
        assert_eq!(ids, [2, 3]);
        let back: Vec<Row<String>> = json::from_json(json::to_json(&all).as_bytes()).unwrap();
        assert_eq!(back, all);
        let mut out = String::new();
        row_json(
            &mut out,
            1,
            &Note {
                title: "x".into(),
                done: false,
            },
        );
        assert_eq!(out, r#"{"id":1,"title":"x","done":false}"#);
        out.clear();
        row_json(&mut out, 4, &Value::Object(Vec::new()));
        assert_eq!(out, r#"{"id":4}"#);
        for json in [r#"{"title":"x"}"#, "{}", "\"b\"", "[1]"] {
            let (mut a, mut b) = (String::new(), String::new());
            row_json(&mut a, 5, &json::parse(json).unwrap());
            splice(&mut b, 5, json);
            assert_eq!(a, b);
        }
    }

    #[test]
    fn pages_newest_first() {
        let t: Table<u32> = Table::new();
        for n in 1..=5 {
            t.add(n);
        }
        let page = |q: &str| {
            t.page(
                &crate::Cx::for_test(&format!("GET /p{q} HTTP/1.1\r\n\r\n"), &[]),
                2,
            )
        };
        let ids = |p: &Page<u32>| p.iter().map(|r| r.id).collect::<Vec<_>>();
        let first = page("");
        assert_eq!((ids(&first), first.number), (vec![5, 4], 1));
        assert_eq!(
            (first.prev.as_deref(), first.next.as_deref()),
            (None, Some("?page=2"))
        );
        let second = page("?q=a+b&page=2&x");
        assert_eq!(ids(&second), [3, 2]);
        assert_eq!(second.prev.as_deref(), Some("?q=a+b&x&page=1"));
        assert_eq!(second.next.as_deref(), Some("?q=a+b&x&page=3"));
        let last = page("?/add&page=3");
        assert_eq!((ids(&last), last.next), (vec![1], None));
        assert_eq!(last.prev.as_deref(), Some("?page=2"));
        let past = page("?page=9");
        assert!(past.is_empty() && past.next.is_none());
        for bad in ["?page=0", "?page=x", "?page=-1"] {
            assert_eq!(page(bad).number, 1, "{bad}");
        }
        let mut n = 0;
        for row in &first {
            n += row.value;
        }
        assert_eq!(n, 9);
        assert_eq!(
            json::to_json(&page("?page=3")),
            r#"{"rows":[{"id":1,"value":1}],"number":3,"prev":"?page=2","next":null}"#
        );
    }

    /// A store in memory, as a database of the app's own would be.
    struct Mem(Mutex<Vec<(String, u64, Option<String>)>>);

    impl Store for Mem {
        fn load(&self, table: &str) -> Result<Vec<(u64, String)>> {
            let mut rows = BTreeMap::new();
            for (t, id, json) in self.0.lock().unwrap().iter() {
                if t == table {
                    match json {
                        Some(j) => rows.insert(*id, j.clone()),
                        None => rows.remove(id),
                    };
                }
            }
            Ok(rows.into_iter().collect())
        }

        fn save(&self, table: &str, id: u64, json: Option<&str>) -> Result {
            if json == Some("\"fail\"") {
                return Err(crate::Error::new(500, "disk full"));
            }
            self.0
                .lock()
                .unwrap()
                .push((table.into(), id, json.map(str::to_string)));
            Ok(())
        }

        /// The cursor is how many changes of any table were made.
        fn changes(&self, table: &str, since: u64) -> Result<Option<store::Changes>> {
            let log = self.0.lock().unwrap();
            let rows = (log.iter().skip(since as usize))
                .filter(|(t, ..)| t == table)
                .map(|(_, id, json)| (*id, json.clone()))
                .collect();
            Ok(Some(store::Changes {
                cursor: log.len() as u64,
                rows,
            }))
        }
    }

    #[test]
    fn saved_tables_come_back() {
        store::memory();
        let mem: &'static Mem = Box::leak(Box::new(Mem(Mutex::new(Vec::new()))));
        let t: Table<String> = Table::saved("saved_tables_come_back");
        t.load(&mut t.rows.write().unwrap(), Some(mem));
        let a = t.add("a".into());
        t.add("b".into());
        t.update(a, |s| s.push('!'));
        t.remove(2);
        let failed =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| t.add("fail".into())));
        assert!(failed.is_err());
        assert!(!t.rows.is_poisoned(), "unlocked before the panic");
        assert!(
            matches!(
                t.rows.read().unwrap_or_else(|e| e.into_inner()).state,
                State::Unread(Some(_))
            ),
            "read again, as the store has it"
        );

        // Read again, as after a restart (the name is the same table's).
        let again: &'static Table<String> =
            Box::leak(Box::new(Table::saved("saved_tables_come_back")));
        NAMES.lock().retain(|(n, _)| *n != "saved_tables_come_back");
        again.load(&mut again.rows.write().unwrap(), Some(mem));
        assert_eq!(again.with(1, String::len), Some(2));
        assert_eq!(
            again.all(),
            [Row {
                id: 1,
                value: "a!".to_string()
            }]
        );
        assert_eq!(again.add("c".into()), 3, "ids go on from the last given");
        let other: Table<String> = Table::saved("saved_tables_come_back");
        let twice = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            other.load(&mut other.rows.write().unwrap(), Some(mem))
        }));
        assert!(twice.is_err(), "two tables of one name");

        // A change that panicked half way: what is saved, not what it left.
        let half = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            again.update(1, |s| {
                s.push('?');
                panic!("half way")
            })
        }));
        assert!(half.is_err());
        assert_eq!(again.with(1, String::clone).as_deref(), Some("a!"));
        assert_eq!(again.add("d".into()), 4);

        // Another instance's rows, read again: the newer of each wins.
        let counts: Table<u64> = Table::saved("refresh_counts");
        counts.load(&mut counts.rows.write().unwrap(), Some(mem));
        counts.add(5);
        mem.save("refresh_counts", 1, Some("7")).unwrap();
        mem.save("refresh_counts", 2, Some("1")).unwrap();
        counts.refresh(|now, stored| stored > now).unwrap();
        assert_eq!(
            (counts.with(1, |n| *n), counts.with(2, |n| *n)),
            (Some(7), Some(1))
        );
        mem.save("refresh_counts", 1, Some("3")).unwrap();
        counts.refresh(|now, stored| stored > now).unwrap();
        assert_eq!(counts.with(1, |n| *n), Some(7), "never back");
        assert_eq!(counts.add(9), 3);

        let random: Table<String> = Table::rest(None, true);
        let id = random.add("x".into());
        assert!(id > 0 && id < 1 << 53 && random.get(id).is_some());
    }

    #[derive(Clone, PartialEq, Debug)]
    struct User {
        email: String,
        age: u32,
    }

    impl Json for User {
        fn json(&self, out: &mut String) {
            out.push_str("{\"email\":");
            self.email.json(out);
            out.push_str(",\"age\":");
            self.age.json(out);
            out.push('}');
        }
    }

    impl FromJson for User {
        fn from_json(v: &Value, p: &mut json::Problems) -> Option<User> {
            Some(User {
                email: p.read("email", v.get("email")?)?,
                age: p.read("age", v.get("age")?)?,
            })
        }
    }

    fn user(email: &str, age: u32) -> User {
        User {
            email: email.into(),
            age,
        }
    }

    #[test]
    fn set_clear_and_unique() {
        let t: Table<User> = Table::new().unique("email", |u: &User| &u.email);
        let a = t.add(user("a@x", 1));
        let b = t.add(user("b@x", 2));
        assert_eq!(t.by("b@x").map(|r| r.id), Some(b));
        assert_eq!(t.by("c@x"), None);
        let err = t.try_add(user("a@x", 3)).unwrap_err();
        assert_eq!((err.status(), err.message()), (422, "email: is taken"));
        assert_eq!(t.len(), 2);
        assert_eq!(t.set(a, user("c@x", 4)), Some(()));
        assert_eq!(t.set(99, user("z@x", 4)), None);
        assert!(
            t.by("a@x").is_none() && t.by("c@x").is_some(),
            "index follows"
        );
        assert!(t.try_set(b, user("c@x", 5)).is_err());
        assert_eq!(
            t.try_set(b, user("b@x", 5)).unwrap(),
            Some(()),
            "its own value"
        );
        assert!(t.try_update(b, |u| u.email = "c@x".into()).is_err());
        assert_eq!(t.get(b).unwrap().age, 5, "left as it was");
        assert_eq!(
            t.try_update(b, |u| u.email = "d@x".into()).unwrap(),
            Some(())
        );
        assert_eq!(t.update(b, |u| u.age += 1), Some(()));
        assert_eq!((t.by("d@x").unwrap().age, t.by("b@x")), (6, None));
        t.remove(b);
        assert!(t.by("d@x").is_none());
        assert!(t.add_unless(|u| u.age == 4, user("e@x", 1)).is_none());
        assert_eq!(t.add(user("d@x", 1)), 3, "ids go on");
        let boom = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| t.add(user("d@x", 9))));
        assert!(boom.is_err() && t.len() == 2, "add panics for a repeat");
        t.clear();
        assert!(t.is_empty() && t.by("c@x").is_none());
        assert_eq!(t.add(user("c@x", 1)), 4, "not given again");
    }

    #[test]
    fn try_update_is_one_step_under_the_lock() {
        static T: Table<u32> = Table::new();
        let id = T.add(0);
        let threads: Vec<_> = (0..4)
            .map(|_| {
                std::thread::spawn(move || {
                    for _ in 0..500 {
                        T.try_update(id, |n| *n += 1).unwrap();
                    }
                })
            })
            .collect();
        threads.into_iter().for_each(|t| t.join().unwrap());
        assert_eq!(T.get(id).unwrap().value, 2000, "no increment is lost");
    }

    #[test]
    fn fresh_empties_the_tables_that_are_ready() {
        static T: Table<u8> = Table::new();
        T.ready();
        T.ready();
        T.add(1);
        crate::test::fresh();
        assert!(T.is_empty());
        let seen = EVERY
            .lock()
            .iter()
            .filter(|t| std::ptr::addr_eq(**t, &T))
            .count();
        assert_eq!(seen, 1, "listed once");
    }

    #[test]
    fn clearing_a_live_table_says_so_once() {
        let t: Table<String> = Table::saved("clear_once").live();
        for n in ["a", "b", "c"] {
            t.add(n.into());
        }
        let mut heard = crate::channel("clear_once").subscribe();
        t.clear();
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .unwrap();
        rt.block_on(async {
            assert_eq!(heard.recv().await.as_deref(), Some("change"));
            let more = tokio::time::timeout(std::time::Duration::from_millis(20), heard.recv());
            assert!(more.await.is_err(), "one message, not one a row");
        });
    }

    #[test]
    fn migrated_live_and_following_a_store() {
        store::memory();
        let mem: &'static Mem = Box::leak(Box::new(Mem(Mutex::new(Vec::new()))));
        mem.save("old_users", 1, Some(r#"{"mail":"a@x"}"#)).unwrap();
        mem.save("old_users", 0, Some("1")).unwrap();
        let t: Table<User> = Table::saved("old_users")
            .unique("email", |u: &User| &u.email)
            .migrate(|row| {
                if let Value::Object(m) = row {
                    for (k, _) in m.iter_mut().filter(|(k, _)| k == "mail") {
                        *k = "email".into();
                    }
                    if m.iter().all(|(k, _)| k != "age") {
                        m.push(("age".into(), Value::Number("7".into())));
                    }
                }
            })
            .live();
        t.load_from(mem);
        assert_eq!(t.by("a@x").unwrap().value, user("a@x", 7));

        // Another instance's changes arrive on a poll, and tell listeners.
        let rt = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        let mut heard = crate::channel("old_users").subscribe();
        t.poll().unwrap();
        mem.save("old_users", 2, Some(r#"{"email":"b@x","age":1}"#))
            .unwrap();
        mem.save("old_users", 1, None).unwrap();
        t.poll().unwrap();
        assert_eq!((t.len(), t.by("a@x")), (1, None));
        assert_eq!(t.by("b@x").unwrap().id, 2);
        rt.block_on(async {
            assert_eq!(heard.recv().await.as_deref(), Some("change"));
        });
        assert_eq!(t.add(user("d@x", 1)), 3);
        // A row that does not read changes nothing, however many came with it.
        mem.save("old_users", 9, Some(r#"{"email":"z@x","age":1}"#))
            .unwrap();
        mem.save("old_users", 2, None).unwrap();
        mem.save("old_users", 8, Some(r#"{"age":"x"}"#)).unwrap();
        assert!(t.poll().is_err());
        assert!(
            t.by("b@x").is_some() && t.by("z@x").is_none(),
            "none applied"
        );

    }
}
