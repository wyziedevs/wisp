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
use crate::{Json, Result};
use std::collections::BTreeMap;
use std::fmt;
use std::sync::{Mutex, MutexGuard};

/// Rows under ids from 1 up (or random ones), kept in order of their ids.
/// A lock whose holder panicked leaves the rows as they were.
pub struct Table<T> {
    rows: Mutex<Rows<T>>,
    /// The name a saved table has in the store; `None` keeps it in memory.
    name: Option<&'static str>,
    codec: Option<Codec<T>>,
    /// Ids are random numbers under 2^53, not counted up.
    random: bool,
}

/// How a saved table's rows become JSON and back.
struct Codec<T> {
    write: fn(&T, &mut String),
    read: fn(&[u8]) -> Result<T>,
}

pub(crate) struct Rows<T> {
    pub(crate) last: u64,
    pub(crate) map: BTreeMap<u64, T>,
    /// Read from the store (or found to have none).
    loaded: bool,
    /// Where changes are saved, once loaded.
    store: Option<&'static dyn Store>,
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
        // `{"id":1,"value":…}`, for a value that is not an object.
        let value = match v.get("value") {
            Some(x) if matches!(v, crate::Value::Object(m) if m.len() == 2) => p.read("value", x),
            _ => T::from_json(v, p),
        }?;
        Some(Row { id, value })
    }
}

/// `{"id":1,…the value's members}`; a value that is not an object is
/// `{"id":1,"value":…}`.
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
static NAMES: Mutex<Vec<(&'static str, usize)>> = Mutex::new(Vec::new());

impl<T> Table<T> {
    /// A table kept in memory: its rows are gone when the server stops.
    pub const fn new() -> Table<T> {
        Table {
            rows: Mutex::new(Rows {
                last: 0,
                map: BTreeMap::new(),
                loaded: false,
                store: None,
            }),
            name: None,
            codec: None,
            random: false,
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
        let mut t = Table::new();
        t.name = name;
        t.codec = Some(Codec {
            write: <T as Json>::json,
            read: crate::json::from_json::<T>,
        });
        t.random = random;
        t
    }

    /// The rows, locked, read from the store the first time.
    pub(crate) fn rows(&self) -> MutexGuard<'_, Rows<T>> {
        let mut rows = self.rows.lock().unwrap_or_else(|e| e.into_inner());
        if !rows.loaded {
            if let Some(store) = store::current() {
                self.load(&mut rows, store);
            }
            rows.loaded = true;
        }
        rows
    }

    /// Reads a saved table's rows from `store`, which keeps its changes
    /// from then on.
    fn load(&self, rows: &mut Rows<T>, store: &'static dyn Store) {
        if let (Some(name), Some(codec)) = (self.name, &self.codec) {
            {
                let mut names = NAMES.lock().unwrap_or_else(|e| e.into_inner());
                let me = self as *const Table<T> as usize;
                match names.iter().find(|(n, _)| *n == name) {
                    Some((_, t)) if *t != me => panic!(
                        "two tables are named `{name}`, and would save over each other: \
                         name one with #[rest(table = \"...\")] or Table::saved(\"...\")"
                    ),
                    Some(_) => {}
                    None => names.push((name, me)),
                }
            }
            let saved = store
                .load(name)
                .unwrap_or_else(|e| panic!("could not read table `{name}`: {}", e.detail()));
            for (id, json) in saved {
                if id == 0 {
                    // The last id given, kept when the row that had it was
                    // removed, so ids are not given twice.
                    rows.last = rows.last.max(json.parse().unwrap_or(0));
                    continue;
                }
                let value = (codec.read)(json.as_bytes()).unwrap_or_else(|e| {
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
            rows.store = Some(store);
        }
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
    pub(crate) fn encode(&self, rows: &Rows<T>, value: &T) -> String {
        let mut json = String::new();
        if let (Some(_), Some(codec)) = (rows.store, &self.codec) {
            (codec.write)(value, &mut json);
        }
        json
    }

    /// Saves row `id` of a saved table: `json` from [`Table::encode`], or
    /// `None` when it is removed. A store that fails panics, which is the
    /// request's 500. What is in memory may then be ahead of the store, so
    /// it is read again on next use.
    pub(crate) fn write(&self, rows: &mut Rows<T>, id: u64, json: Option<&str>) {
        let (Some(store), Some(name)) = (rows.store, self.name) else {
            return;
        };
        if let Err(e) = store.save(name, id, json) {
            rows.map.clear();
            rows.last = 0;
            rows.loaded = false;
            rows.store = None;
            panic!("could not save row {id} of table `{name}`: {}", e.detail());
        }
    }

    /// Keeps `value` under a new id, which it returns.
    pub fn add(&self, value: T) -> u64 {
        let mut rows = self.rows();
        let id = self.next_id(&mut rows);
        let json = self.encode(&rows, &value);
        self.write(&mut rows, id, Some(&json));
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
        self.write(rows, id, None);
        if id == rows.last && !self.random {
            self.write(rows, 0, Some(&id.to_string()));
        }
        rows.map.remove(&id)
    }

    /// Takes the row out; `None` when there is none.
    pub fn remove(&self, id: u64) -> Option<T> {
        self.delete(&mut self.rows(), id)
    }

    /// Changes the row in place: `NOTES.update(id, |n| n.done = true)`.
    /// `None` when there is none.
    pub fn update<R>(&self, id: u64, f: impl FnOnce(&mut T) -> R) -> Option<R> {
        let mut rows = self.rows();
        let r = f(rows.map.get_mut(&id)?);
        let json = self.encode(&rows, &rows.map[&id]);
        self.write(&mut rows, id, Some(&json));
        Some(r)
    }

    pub fn len(&self) -> usize {
        self.rows().map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rows().map.is_empty()
    }
}

impl<T: Clone> Table<T> {
    /// A copy of the row.
    pub fn get(&self, id: u64) -> Option<Row<T>> {
        let rows = self.rows();
        let value = rows.map.get(&id)?.clone();
        Some(Row { id, value })
    }

    /// A copy of every row, in order of their ids (oldest first, unless
    /// the ids are random).
    pub fn all(&self) -> Vec<Row<T>> {
        self.filter(|_| true)
    }

    /// A copy of the first row for which `f` is true.
    pub fn find(&self, f: impl Fn(&T) -> bool) -> Option<Row<T>> {
        let rows = self.rows();
        let (&id, v) = rows.map.iter().find(|(_, v)| f(v))?;
        Some(Row {
            id,
            value: v.clone(),
        })
    }

    /// A copy of every row for which `f` is true: `NOTES.filter(|n| !n.done)`.
    pub fn filter(&self, f: impl Fn(&T) -> bool) -> Vec<Row<T>> {
        let rows = self.rows();
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
        let mut rows = t.rows.lock().unwrap();
        t.load(&mut rows, mem);
        rows.loaded = true;
        drop(rows);
        let a = t.add("a".into());
        t.add("b".into());
        t.update(a, |s| s.push('!'));
        t.remove(2);
        let failed =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| t.add("fail".into())));
        assert!(failed.is_err());
        assert!(
            !t.rows.lock().unwrap_or_else(|e| e.into_inner()).loaded,
            "read again, as the store has it"
        );

        // Read again, as after a restart (the name is the same table's).
        let again: &'static Table<String> =
            Box::leak(Box::new(Table::saved("saved_tables_come_back")));
        {
            let mut names = NAMES.lock().unwrap();
            names.retain(|(n, _)| *n != "saved_tables_come_back");
        }
        let mut rows = again.rows.lock().unwrap();
        again.load(&mut rows, mem);
        rows.loaded = true;
        drop(rows);
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
            other.load(&mut other.rows.lock().unwrap(), mem)
        }));
        assert!(twice.is_err(), "two tables of one name");

        let random: Table<String> = Table::rest(None, true);
        let id = random.add("x".into());
        assert!(id > 0 && id < 1 << 53 && random.get(id).is_some());
    }
}
