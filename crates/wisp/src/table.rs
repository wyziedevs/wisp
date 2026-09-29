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
use crate::{Json, Result, Shared};
use std::collections::BTreeMap;
use std::fmt;
use std::sync::{RwLock, RwLockReadGuard, RwLockWriteGuard};

/// Rows under ids from 1 up (or random ones), kept in order of their ids.
/// Reads share the lock; a change has it alone. A lock whose holder
/// panicked leaves the rows as they were.
pub struct Table<T> {
    rows: RwLock<Rows<T>>,
    /// Where a saved table keeps its rows; `None` keeps them in memory.
    saved: Option<Saved<T>>,
    /// Ids are random numbers under 2^53, not counted up.
    random: bool,
}

/// A saved table's name in the store, and how its rows become JSON and back.
struct Saved<T> {
    name: &'static str,
    write: fn(&T, &mut String),
    read: fn(&[u8]) -> Result<T>,
}

pub(crate) struct Rows<T> {
    pub(crate) last: u64,
    pub(crate) map: BTreeMap<u64, T>,
    state: State,
}

#[derive(Clone, Copy)]
enum State {
    /// A saved table whose rows are not read from the store yet.
    Unread,
    /// Kept in memory only.
    Memory,
    /// Read from the store, which keeps each change.
    Stored(&'static dyn Store),
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

impl<T> Table<T> {
    /// A table kept in memory: its rows are gone when the server stops.
    pub const fn new() -> Table<T> {
        Table::make(None, false)
    }

    const fn make(saved: Option<Saved<T>>, random: bool) -> Table<T> {
        let state = match saved {
            Some(_) => State::Unread,
            None => State::Memory,
        };
        Table {
            rows: RwLock::new(Rows {
                last: 0,
                map: BTreeMap::new(),
                state,
            }),
            saved,
            random,
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
                write: <T as Json>::json,
                read: crate::json::from_json::<T>,
            }),
            None => None,
        };
        Table::make(saved, random)
    }

    /// The rows, locked for reading, read from the store the first time.
    pub(crate) fn read(&self) -> RwLockReadGuard<'_, Rows<T>> {
        loop {
            let rows = self.rows.read().unwrap_or_else(|e| e.into_inner());
            if !matches!(rows.state, State::Unread) {
                return rows;
            }
            drop(rows);
            drop(self.write());
        }
    }

    /// The rows, locked for a change, read from the store the first time.
    pub(crate) fn write(&self) -> RwLockWriteGuard<'_, Rows<T>> {
        let mut rows = self.rows.write().unwrap_or_else(|e| e.into_inner());
        if matches!(rows.state, State::Unread) {
            self.load(&mut rows, store::current());
        }
        rows
    }

    /// Reads a saved table's rows from the store now, rather than on first
    /// use: at startup, after `init` has set the store. A store that fails
    /// then stops the server before it answers anything.
    #[doc(hidden)]
    pub fn ready(&self) {
        drop(self.write());
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
            let value = (saved.read)(json.as_bytes()).unwrap_or_else(|e| {
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

    /// The JSON a saved table keeps for `value`; empty for one in memory.
    fn encode(&self, rows: &Rows<T>, value: &T) -> String {
        let mut json = String::new();
        if let (State::Stored(_), Some(saved)) = (rows.state, &self.saved) {
            (saved.write)(value, &mut json);
        }
        json
    }

    /// Saves row `id` of a saved table: `json` is its value's JSON, `None`
    /// when it is removed. A store that fails panics, which is the
    /// request's 500. What is in memory may then be ahead of the store, so
    /// it is read again on next use.
    pub(crate) fn save(&self, rows: &mut Rows<T>, id: u64, json: Option<&str>) {
        let (State::Stored(store), Some(saved)) = (rows.state, &self.saved) else {
            return;
        };
        if let Err(e) = store.save(saved.name, id, json) {
            rows.map.clear();
            rows.last = 0;
            rows.state = State::Unread;
            panic!(
                "could not save row {id} of table `{}`: {}",
                saved.name,
                e.detail()
            );
        }
    }

    /// Keeps `value` under a new id, which it returns.
    pub fn add(&self, value: T) -> u64 {
        let mut rows = self.write();
        let id = self.next_id(&mut rows);
        let json = self.encode(&rows, &value);
        self.save(&mut rows, id, Some(&json));
        rows.map.insert(id, value);
        id
    }

    /// Saves that row `id` is gone and takes it out. When it had the last
    /// id given, the store keeps that id as row 0, so it is not given again
    /// after a restart.
    pub(crate) fn delete(&self, rows: &mut Rows<T>, id: u64) -> Option<T> {
        if !rows.map.contains_key(&id) {
            return None;
        }
        self.save(rows, id, None);
        if id == rows.last && !self.random {
            self.save(rows, 0, Some(&id.to_string()));
        }
        rows.map.remove(&id)
    }

    /// Takes the row out; `None` when there is none.
    pub fn remove(&self, id: u64) -> Option<T> {
        self.delete(&mut self.write(), id)
    }

    /// Changes the row in place: `NOTES.update(id, |n| n.done = true)`.
    /// `None` when there is none.
    pub fn update<R>(&self, id: u64, f: impl FnOnce(&mut T) -> R) -> Option<R> {
        let mut rows = self.write();
        let r = f(rows.map.get_mut(&id)?);
        let json = self.encode(&rows, &rows.map[&id]);
        self.save(&mut rows, id, Some(&json));
        Some(r)
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
        assert!(
            matches!(
                t.rows.read().unwrap_or_else(|e| e.into_inner()).state,
                State::Unread
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

        let random: Table<String> = Table::rest(None, true);
        let id = random.add("x".into());
        assert!(id > 0 && id < 1 << 53 && random.get(id).is_some());
    }
}
