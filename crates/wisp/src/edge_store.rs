//! Durable tables on the edge: with `WISP_STORE` set, the saved tables
//! (`#[derive(Rest)]`, `Table::saved`) live where it says, and the host's
//! bridge does the talking: `d1:BINDING` (Cloudflare D1), `deno-kv` (Deno
//! KV), or `libsql://…` and `https://…` (Turso or any libSQL server over
//! HTTP, with `WISP_STORE_TOKEN`).
//!
//! [`Store`] is synchronous and the host is not, so every row is read once,
//! when the instance starts, and changes are queued as they are made and
//! written in one batch, in order, before the request that made them is
//! answered. A batch that fails answers 500 and retires the instance (a
//! trap), so the next one starts from what the store holds.
//!
//! Rows cross as lines, `table\tid\tjson`; an empty `json` removes the row.

use crate::{Error, Request, Result, Store};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll, Waker};

/// Where the bridge finds the store: `GET` reads every row, `POST` writes.
const URL: &str = "wisp:store";

/// Each table's rows, until it reads them: `None` once it has.
type Tables = HashMap<String, Option<Vec<(u64, String)>>>;

thread_local! {
    static ROWS: RefCell<Tables> = RefCell::new(HashMap::new());
    /// Changes not yet sent.
    static QUEUE: RefCell<String> = const { RefCell::new(String::new()) };
    static SENDING: Cell<bool> = const { Cell::new(false) };
    static FAILED: Cell<bool> = const { Cell::new(false) };
    static WAITING: RefCell<Vec<Waker>> = const { RefCell::new(Vec::new()) };
}

/// Reads every row from the store `WISP_STORE` names, and keeps the saved
/// tables there from now on. Run before `init`, which may set a store of
/// its own instead.
pub(crate) async fn open() -> Result {
    let reply = crate::edge::fetch(Request::new("GET", URL)).await?;
    let text = std::str::from_utf8(reply.bytes())
        .map_err(|_| Error::new(500, "the store sent rows that are not UTF-8"))?;
    if reply.status != 200 {
        return Err(Error::new(500, format!("could not read the rows: {text}")));
    }
    let mut tables = Tables::new();
    for line in text.lines().filter(|l| !l.is_empty()) {
        let (table, id, json) = split(line).ok_or_else(|| {
            Error::new(
                500,
                format!("the store sent a row Wisp cannot read: {line}"),
            )
        })?;
        tables
            .entry(table.into())
            .or_insert_with(|| Some(Vec::new()))
            .get_or_insert_default()
            .push((id, json.into()));
    }
    ROWS.set(tables);
    crate::store(Edge);
    Ok(())
}

/// `table\tid\tjson`.
fn split(line: &str) -> Option<(&str, u64, &str)> {
    let (table, rest) = line.split_once('\t')?;
    let (id, json) = rest.split_once('\t')?;
    Some((table, id.parse().ok()?, json))
}

struct Edge;

impl Store for Edge {
    fn load(&self, table: &str) -> Result<Vec<(u64, String)>> {
        if table.contains(['\t', '\n']) {
            return Err(Error::new(500, format!("`{table}` cannot name a table")));
        }
        // A second read would find nothing: the instance starts over instead.
        ROWS.with_borrow_mut(|rows| match rows.insert(table.into(), None) {
            Some(None) => Err(Error::new(500, format!("table `{table}` was read already"))),
            Some(Some(kept)) => Ok(kept),
            None => Ok(Vec::new()),
        })
    }

    fn save(&self, table: &str, id: u64, json: Option<&str>) -> Result {
        QUEUE.with_borrow_mut(|q| {
            for part in [table, "\t", &id.to_string(), "\t", json.unwrap_or(""), "\n"] {
                q.push_str(part);
            }
        });
        Ok(())
    }
}

/// Whether there are changes to send and nothing is sending them.
pub(crate) fn due() -> bool {
    !SENDING.get() && !FAILED.get() && QUEUE.with_borrow(|q| !q.is_empty())
}

/// Sends the changes queued so far, as one batch. One that fails traps,
/// once whoever waits on [`Saved`] has been told.
pub(crate) async fn send() {
    SENDING.set(true);
    let mut req = Request::new("POST", URL);
    req.body = QUEUE.take().into_bytes();
    let sent = match crate::edge::fetch(req).await {
        Ok(r) if r.status == 200 => Ok(()),
        Ok(r) => Err(String::from_utf8_lossy(r.bytes()).into_owned()),
        Err(e) => Err(e.message().to_string()),
    };
    SENDING.set(false);
    FAILED.set(sent.is_err());
    WAITING.take().into_iter().for_each(Waker::wake);
    if let Err(why) = sent {
        panic!("could not save to WISP_STORE: {why}");
    }
}

/// Ready once every change made so far is in the store: `false` if it
/// could not be.
pub(crate) struct Saved;

impl Future for Saved {
    type Output = bool;

    fn poll(self: Pin<&mut Self>, cx: &mut Context) -> Poll<bool> {
        if FAILED.get() {
            return Poll::Ready(false);
        }
        if !SENDING.get() && QUEUE.with_borrow(String::is_empty) {
            return Poll::Ready(true);
        }
        WAITING.with_borrow_mut(|w| w.push(cx.waker().clone()));
        Poll::Pending
    }
}
