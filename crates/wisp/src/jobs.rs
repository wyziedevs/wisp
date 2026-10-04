//! Jobs: work that outlives its request, kept in a table so a restart does
//! not lose it, and a schedule written as cron.
//!
//! ```ignore
//! // src/hooks.rs
//! async fn init() -> Result {
//!     wisp::work("mail", |m: Mail| async move { send(&m).await }); // Err: tried again
//!     wisp::cron("0 3 * * *", || async { db::purge().await });      // 03:00 UTC daily
//!     Ok(())
//! }
//! // anywhere: wisp::queue("mail").push(&Mail { to, body });
//! ```
//!
//! A job that fails (an `Err`, or a panic) is tried again after 4, 8, 16, 32
//! seconds and so on (an hour at most), five tries in all; then it stays in
//! the queue's table, `dead`, with its last error, for you to look at. A
//! job is run at least once: one that was running when the process died is
//! run again a minute later. Jobs of a queue run one at a time, in the order
//! of their ids. Several servers of one app may work a queue if they share a
//! store that all see changes of (`WISP_STORE_POLL`); each job is claimed.
//!
//! The edge and serverless builds (`wisp build --target cloudflare`, `vercel`,
//! `netlify`) have no background, so the same code runs from the host's cron
//! trigger: `wisp build` reads each `wisp::cron("...")` of `src/` (its
//! schedule a string literal) and writes it into the host's config
//! (`wrangler.toml` `[triggers]`, `vercel.json` `crons`, a Netlify scheduled
//! function). A trigger runs the tasks of its schedule, then every queue's due
//! jobs, so an app with `work` gets a trigger each minute. The trigger is a
//! request to `/_wisp/cron/<schedule>` that needs `Authorization: Bearer
//! $CRON_SECRET`; without that variable set it answers 404. Queued jobs live in
//! the app's table, so set `WISP_STORE` there.

use crate::json::{FromJson, Problems};
use crate::{Json, Table, Value};
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};

/// Tries a job gets, the first included.
const TRIES: u32 = 5;
/// How long a claimed job is left to its worker before another may take it.
const LEASE: u64 = 60;

/// A job in its queue's table.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Job {
    /// The job's JSON.
    pub(crate) payload: String,
    pub(crate) tries: u32,
    /// Not before this time (unix seconds).
    pub(crate) run_at: u64,
    /// Claimed by a worker until this time.
    pub(crate) lease: u64,
    /// Out of tries: kept, not run.
    pub(crate) dead: bool,
    pub(crate) error: Option<String>,
}

impl Json for Job {
    fn json(&self, out: &mut String) {
        out.push_str("{\"payload\":");
        self.payload.json(out);
        out.push_str(",\"tries\":");
        self.tries.json(out);
        out.push_str(",\"run_at\":");
        self.run_at.json(out);
        out.push_str(",\"lease\":");
        self.lease.json(out);
        out.push_str(",\"dead\":");
        self.dead.json(out);
        out.push_str(",\"error\":");
        self.error.json(out);
        out.push('}');
    }
}

impl FromJson for Job {
    fn from_json(v: &Value, p: &mut Problems) -> Option<Job> {
        Some(Job {
            payload: p.read("payload", v.get("payload")?)?,
            tries: p.read("tries", v.get("tries")?)?,
            run_at: p.read("run_at", v.get("run_at")?)?,
            lease: p.read("lease", v.get("lease")?)?,
            dead: p.read("dead", v.get("dead")?)?,
            error: match v.get("error") {
                Some(e) => p.read("error", e)?,
                None => None,
            },
        })
    }
}

/// A named queue of jobs, from [`queue`].
pub struct Queue {
    name: &'static str,
    table: Table<Job>,
    wake: tokio::sync::Notify,
    worked: AtomicBool,
}

/// The queue called `name` (letters, digits, `_`, `-`), made on first use;
/// its jobs are the saved table `queue-name`.
pub fn queue(name: &str) -> &'static Queue {
    static ALL: crate::Shared<Vec<(&'static str, &'static Queue)>> = crate::Shared::new(Vec::new());
    let mut all = ALL.lock();
    if let Some((_, q)) = all.iter().find(|(n, _)| *n == name) {
        return q;
    }
    assert!(
        !name.is_empty()
            && name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-'),
        "a queue's name is letters, digits, _ and -: {name:?}"
    );
    let name: &'static str = Box::leak(name.into());
    let table: &'static str = Box::leak(format!("queue-{name}").into());
    let q: &'static Queue = Box::leak(Box::new(Queue {
        name,
        table: Table::saved(table),
        wake: tokio::sync::Notify::new(),
        worked: AtomicBool::new(false),
    }));
    all.push((name, q));
    q
}

impl Queue {
    /// Adds a job, to run as soon as a worker is free; returns its id.
    pub fn push(&self, job: &impl Json) -> u64 {
        self.later(0, job)
    }

    /// Adds a job to run no sooner than `secs` seconds from now.
    pub fn later(&self, secs: u64, job: &impl Json) -> u64 {
        let id = self.table.add(Job {
            payload: crate::to_json(job),
            tries: 0,
            run_at: crate::unix_now() + secs,
            lease: 0,
            dead: false,
            error: None,
        });
        self.wake.notify_one();
        id
    }

    /// Jobs waiting, running or being tried again (not the dead).
    pub fn pending(&self) -> usize {
        self.table.filter(|j| !j.dead).len()
    }

    /// Jobs out of tries, with the last error each had, for a page or a log.
    pub fn dead(&self) -> Vec<(u64, String)> {
        let dead = self.table.filter(|j| j.dead);
        dead.into_iter()
            .map(|r| (r.id, r.error.clone().unwrap_or_default()))
            .collect()
    }

    /// Puts a dead job back to be tried again from the start.
    pub fn retry(&self, id: u64) -> bool {
        let revived = self.table.update(id, |j| {
            let was = j.dead;
            *j = Job {
                dead: false,
                tries: 0,
                run_at: 0,
                lease: 0,
                ..j.clone()
            };
            was
        });
        self.wake.notify_one();
        revived == Some(true)
    }
}

/// Seconds before try number `tries` (1 for the second): 4, 8, 16… an hour at most.
fn backoff(tries: u32) -> u64 {
    (2u64 << tries.min(11)).min(3600)
}

/// What a failed try does to the job; `fatal` ones, which no try would
/// mend, are dead at once.
fn settle(j: &mut Job, now: u64, error: String, fatal: bool) {
    // On the edge the try was counted when it was claimed.
    if cfg!(not(target_arch = "wasm32")) {
        j.tries += 1;
    }
    j.lease = 0;
    if fatal || j.tries >= TRIES {
        j.dead = true;
    }
    j.run_at = now + backoff(j.tries);
    j.error = Some(error);
}

/// How one try of a job ended.
enum Outcome {
    Done,
    /// To be tried again later.
    Failed(String),
    /// No try would mend it: dead at once.
    Fatal(String),
}

type Tried = Pin<Box<dyn Future<Output = Outcome> + Send>>;

/// A queue's worker as the table sees it: a job's payload to its try.
type Runner = Box<dyn FnMut(&str) -> Tried + Send>;

/// `f` as a [`Runner`]: a payload that is not a `T` is fatal.
fn runner<T, F, Fut>(mut f: F) -> Runner
where
    T: FromJson + Send + 'static,
    F: FnMut(T) -> Fut + Send + 'static,
    Fut: Future<Output = crate::Result> + Send + 'static,
{
    Box::new(
        move |payload| match crate::from_json::<T>(payload.as_bytes()) {
            Err(e) => {
                let why = format!("not a job of this worker: {}", e.message());
                Box::pin(std::future::ready(Outcome::Fatal(why)))
            }
            Ok(job) => Box::pin(attempt(f(job))),
        },
    )
}

/// A try on its own task, so a panic fails the job and not the worker.
#[cfg(not(target_arch = "wasm32"))]
async fn attempt(fut: impl Future<Output = crate::Result> + Send + 'static) -> Outcome {
    match tokio::spawn(fut).await {
        Ok(Ok(())) => Outcome::Done,
        Ok(Err(e)) => Outcome::Failed(e.detail()),
        Err(e) => Outcome::Failed(format!("panic: {e}")),
    }
}

/// A panic traps the edge instance (see [`claim`] for what keeps such a job
/// from being run for ever).
#[cfg(target_arch = "wasm32")]
async fn attempt(fut: impl Future<Output = crate::Result> + Send + 'static) -> Outcome {
    match fut.await {
        Ok(()) => Outcome::Done,
        Err(e) => Outcome::Failed(e.detail()),
    }
}

/// Runs `f` on each job pushed on the queue `name`, one at a time, from
/// now on: `wisp::work("mail", |m: Mail| async move { send(&m).await })`.
/// `Ok` ends the job; `Err` or a panic has it tried again later (see the
/// module). A job that is not a `T` is dead at once. Call it once per queue,
/// in `init`.
///
/// On the edge and on serverless hosts there is no background, so the jobs
/// are run when the host's cron trigger comes (see the module).
pub fn work<T, F, Fut>(name: &str, f: F)
where
    T: FromJson + Send + 'static,
    F: FnMut(T) -> Fut + Send + 'static,
    Fut: Future<Output = crate::Result> + Send + 'static,
{
    let q = queue(name);
    assert!(
        !q.worked.swap(true, Ordering::Relaxed),
        "queue {name:?} already has a worker"
    );
    #[cfg_attr(target_arch = "wasm32", allow(unused_mut))]
    let mut run = runner(f);
    #[cfg(target_arch = "wasm32")]
    HOSTED.lock().push((q, run));
    #[cfg(not(target_arch = "wasm32"))]
    crate::spawn(async move {
        loop {
            if !step(q, &mut run).await {
                return;
            }
        }
    });
}

/// What [`claim`] found.
enum Claim {
    Job(crate::Row<Job>),
    /// Nothing is due, or another worker took it: when the next job is due
    /// (`u64::MAX`: none).
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    Idle(u64),
}

/// The first job of `q` that is due, claimed for [`LEASE`] seconds.
///
/// On the edge a try that panics traps the instance and is never settled,
/// so there a try counts when it is claimed: a job claimed [`TRIES`] times
/// without being settled is dead, and does not trap the host for ever.
fn claim(q: &Queue, now: u64) -> Claim {
    // The first job due, and when the next that is not will be.
    let (mut due, mut next) = (None, u64::MAX);
    q.table.each(|id, j| {
        let at = j.run_at.max(j.lease);
        if j.dead {
        } else if at > now {
            next = next.min(at);
        } else if due.is_none() {
            due = Some(crate::Row {
                id,
                value: j.clone(),
            });
        }
    });
    let Some(row) = due else {
        return Claim::Idle(next);
    };
    let claimed = q.table.update(row.id, |j| {
        if j.dead || j.lease > now {
            return false;
        }
        if cfg!(target_arch = "wasm32") {
            if j.tries >= TRIES {
                j.dead = true;
                j.error = Some("it stopped the instance every time".into());
                return false;
            }
            j.tries += 1;
        }
        j.lease = now + LEASE;
        true
    });
    match claimed {
        Some(true) => Claim::Job(row),
        _ => Claim::Idle(next),
    }
}

/// Records how the claimed job `id` of `q`, which had `tries` before, went:
/// gone if it is done, else kept for another try, or dead.
fn settled(q: &Queue, id: u64, tries: u32, outcome: Outcome) {
    let (error, fatal) = match outcome {
        Outcome::Done => {
            q.table.remove(id);
            return;
        }
        Outcome::Failed(e) => (e, false),
        Outcome::Fatal(e) => (e, true),
    };
    crate::http::log(format_args!(
        "wisp: job {id} of queue {} failed (try {}): {error}",
        q.name,
        tries + 1
    ));
    q.table
        .update(id, |j| settle(j, crate::unix_now(), error, fatal));
}

/// Runs the next due job of `q`, or waits for one: `false` once the server is stopping.
#[cfg(not(target_arch = "wasm32"))]
async fn step(q: &'static Queue, run: &mut Runner) -> bool {
    let now = crate::unix_now();
    let next = match claim(q, now) {
        Claim::Job(row) => {
            let outcome = run(&row.payload).await;
            settled(q, row.id, row.tries, outcome);
            return true;
        }
        Claim::Idle(next) => next,
    };
    // Nothing due: a push wakes it, else when the next job is due, or in
    // a second, when other servers may push through the store.
    let mut wait = std::time::Duration::from_secs(next.saturating_sub(now).max(1));
    if next == u64::MAX {
        wait = std::time::Duration::MAX;
    }
    if crate::env_or("WISP_STORE_POLL", 0u64) > 0 {
        wait = wait.min(std::time::Duration::from_secs(1));
    }
    crate::http::first(
        async {
            crate::http::first(async { q.wake.notified().await }, async {
                match wait == std::time::Duration::MAX {
                    true => std::future::pending().await,
                    false => tokio::time::sleep(wait).await,
                }
            })
            .await;
            true
        },
        async {
            crate::http::stopped().await;
            false
        },
    )
    .await
}

/// The workers of a host that has no background (the edge, serverless):
/// run when its trigger comes, by [`drain`].
#[cfg(any(target_arch = "wasm32", test))]
static HOSTED: crate::Shared<Vec<(&'static Queue, Runner)>> = crate::Shared::new(Vec::new());

/// Runs every job that is due, of every queue that has a worker.
#[cfg(any(target_arch = "wasm32", test))]
pub(crate) async fn drain() {
    let queues = HOSTED.lock().len();
    for i in 0..queues {
        let q = HOSTED.lock()[i].0;
        while let Claim::Job(row) = claim(q, crate::unix_now()) {
            // Called with the lock held, awaited without it.
            let tried = (HOSTED.lock()[i].1)(&row.payload);
            settled(q, row.id, row.tries, tried.await);
        }
    }
}

/// A cron task of a host that has no background: run when its trigger comes.
#[cfg(any(target_arch = "wasm32", test))]
type Hosted = Box<dyn FnMut() -> Pin<Box<dyn Future<Output = ()> + Send>> + Send>;

/// The schedules of the app, by their [`wisp_shared::cron_slug`].
#[cfg(any(target_arch = "wasm32", test))]
static SCHEDULED: crate::Shared<Vec<(String, Hosted)>> = crate::Shared::new(Vec::new());

/// What a host's trigger does: `slug` is the schedule that fired (see
/// [`wisp_shared::cron_slug`]). Runs its tasks one after the other, then
/// the queues' due jobs.
#[cfg(any(target_arch = "wasm32", test))]
async fn fire(slug: &str) {
    let n = SCHEDULED.lock().len();
    for i in 0..n {
        let task = {
            let mut all = SCHEDULED.lock();
            let (at, task) = &mut all[i];
            (at == slug).then(task)
        };
        if let Some(task) = task {
            task.await;
        }
    }
    drain().await;
}

/// The reply to a request for `/_wisp/cron/<slug>`: the host's trigger.
/// Only with `Authorization: Bearer <CRON_SECRET>` (the variable Vercel
/// sends its crons with); else it is not there, and without that variable
/// set nothing is.
#[cfg(target_arch = "wasm32")]
pub(crate) async fn trigger(cx: &crate::Cx) -> crate::Reply {
    let slug = cx.path().strip_prefix("/_wisp/cron/").unwrap_or("");
    let secret = crate::env("CRON_SECRET").filter(|s| !s.is_empty());
    let sent = cx.bearer().unwrap_or("");
    match secret {
        Some(s) if crate::secure_eq(&s, sent) => {
            fire(slug).await;
            crate::Reply::plain(204)
        }
        _ => crate::Reply::plain(404),
    }
}

/// Days since 1970-01-01 to (year, month 1-12, day 1-31).
fn civil(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

/// A cron expression: minute, hour, day of month, month, day of week
/// (0 or 7 is Sunday), each `*`, a number, `a-b`, `*/n`, `a-b/n` or a
/// comma list of those. A day matches when its day of month and its day
/// of week both do, or either, if both are restricted (neither starts
/// with `*`, as in Vixie cron: `*/2` restricts nothing for this rule).
#[derive(Debug, PartialEq)]
pub(crate) struct Cron {
    minute: u64,
    hour: u64,
    dom: u64,
    month: u64,
    dow: u64,
    both: bool,
}

impl Cron {
    pub(crate) fn parse(expr: &str) -> Result<Cron, String> {
        let f: Vec<&str> = expr.split_whitespace().collect();
        let [m, h, dom, mon, dow] = f[..] else {
            return Err(format!(
                "{expr:?} is not 5 fields: minute hour day month weekday"
            ));
        };
        let field = |s: &str, lo: u32, hi: u32| -> Result<u64, String> {
            let mut set = 0u64;
            for part in s.split(',') {
                let (range, step) = part.split_once('/').unwrap_or((part, "1"));
                let step: u32 = step
                    .parse()
                    .ok()
                    .filter(|&n| n > 0)
                    .ok_or(format!("bad step in {s:?}"))?;
                let num = |x: &str| x.parse::<u32>().ok().filter(|n| (lo..=hi).contains(n));
                let (a, b) = match range.split_once('-') {
                    _ if range == "*" => (lo, hi),
                    Some((a, b)) => (
                        num(a).ok_or(format!("bad {s:?}"))?,
                        num(b).ok_or(format!("bad {s:?}"))?,
                    ),
                    None => {
                        let a = num(range).ok_or(format!("{s:?} is outside {lo}-{hi}"))?;
                        (a, if part.contains('/') { hi } else { a })
                    }
                };
                if a > b {
                    return Err(format!("{s:?} runs backwards"));
                }
                set |= (a..=b).step_by(step as usize).fold(0, |s, n| s | 1u64 << n);
            }
            Ok(set)
        };
        let mut dow_set = field(dow, 0, 7)?;
        dow_set = (dow_set | dow_set >> 7) & 0x7f;
        Ok(Cron {
            minute: field(m, 0, 59)?,
            hour: field(h, 0, 23)?,
            dom: field(dom, 1, 31)?,
            month: field(mon, 1, 12)?,
            dow: dow_set,
            // As Vixie cron: a field that starts with `*` (`*/2` too) is not
            // a restriction, so the other day field alone decides.
            both: !dom.starts_with('*') && !dow.starts_with('*'),
        })
    }

    fn day_ok(&self, days: i64) -> bool {
        let (_, m, d) = civil(days);
        // 1970-01-01 was a Thursday (4).
        let w = (days + 4).rem_euclid(7);
        let (dom, dow) = (self.dom >> d & 1 == 1, self.dow >> w & 1 == 1);
        self.month >> m & 1 == 1 && if self.both { dom || dow } else { dom && dow }
    }

    /// The first minute after `after` (unix seconds) that matches, as unix
    /// seconds; `None` if none comes in the next 8 years (February 30).
    pub(crate) fn next(&self, after: u64) -> Option<u64> {
        let mut t = (after / 60 + 1) * 60;
        let end = t + 8 * 366 * 86_400;
        while t < end {
            let (days, secs) = ((t / 86_400) as i64, t % 86_400);
            if !self.day_ok(days) {
                t = (days as u64 + 1) * 86_400;
            } else if self.hour >> (secs / 3600) & 1 == 0 {
                t = t / 3600 * 3600 + 3600;
            } else if self.minute >> (secs / 60 % 60) & 1 == 0 {
                t += 60;
            } else {
                return Some(t);
            }
        }
        None
    }
}

/// Runs `task` whenever `expr` (a cron expression, in UTC: see [`Cron`] for
/// the fields, `"0 3 * * *"` is 03:00 daily) says, until the server stops.
/// Panics at once, in `init`, for an expression that is wrong. A run that
/// takes longer than the gap to the next skips the minutes it overran; one
/// that panics runs again at the next match, as with [`every`](crate::every).
/// Several servers each run it: have one do it (a job on a [`queue`] once).
/// On the edge and serverless hosts the host's trigger runs it (see the
/// module), so `expr` must be a string literal in the source.
pub fn cron<F, Fut>(expr: &str, mut task: F)
where
    F: FnMut() -> Fut + Send + 'static,
    Fut: Future<Output = ()> + Send + 'static,
{
    let cron = Cron::parse(expr).unwrap_or_else(|e| panic!("wisp::cron: {e}"));
    assert!(cron.next(0).is_some(), "wisp::cron: {expr:?} never comes");
    #[cfg(target_arch = "wasm32")]
    SCHEDULED.lock().push((
        wisp_shared::cron_slug(expr),
        Box::new(move || Box::pin(task())),
    ));
    #[cfg(not(target_arch = "wasm32"))]
    crate::spawn(async move {
        loop {
            let now = crate::unix_now();
            let Some(at) = cron.next(now) else { return };
            let wait = std::time::Duration::from_secs(at - now);
            let stopped = crate::http::first(
                async {
                    tokio::time::sleep(wait).await;
                    false
                },
                async {
                    crate::http::stopped().await;
                    true
                },
            )
            .await;
            if stopped {
                return;
            }
            // A panic would end this loop, and with it every later run.
            let _ = tokio::spawn(task()).await;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    const JAN1_2024: u64 = 1_704_067_200; // a Monday, 00:00 UTC

    fn next(expr: &str, after: u64) -> u64 {
        Cron::parse(expr).unwrap().next(after).unwrap() - JAN1_2024
    }

    #[test]
    fn cron_finds_the_next_minute() {
        const H: u64 = 3600;
        const D: u64 = 86_400;
        assert_eq!(next("0 3 * * *", JAN1_2024), 3 * H);
        assert_eq!(
            next("0 3 * * *", JAN1_2024 + 3 * H),
            D + 3 * H,
            "after, not at"
        );
        assert_eq!(next("* * * * *", JAN1_2024), 60);
        assert_eq!(next("*/15 * * * *", JAN1_2024 + 61), 15 * 60);
        assert_eq!(next("5,10 1-2 * * *", JAN1_2024), H + 5 * 60);
        assert_eq!(next("0 9 * * 1", JAN1_2024), 9 * H, "Monday");
        assert_eq!(next("0 9 * * 0", JAN1_2024), 6 * D + 9 * H, "Sunday");
        assert_eq!(next("0 9 * * 7", JAN1_2024), 6 * D + 9 * H, "Sunday, as 7");
        assert_eq!(
            next("30 2 29 2 *", JAN1_2024),
            (31 + 28) * D + 2 * H + 30 * 60,
            "leap day"
        );
        assert_eq!(next("0 0 1 6 *", JAN1_2024), 152 * D, "June 1st");
        // Both day fields given: either one.
        assert_eq!(
            next("0 0 3 * 2", JAN1_2024),
            D,
            "Tuesday the 2nd, before the 3rd"
        );
        assert_eq!(
            next("0 0 */2 * 1", JAN1_2024),
            14 * D,
            "a starred day field is not a restriction: odd days that are Mondays"
        );
        assert_eq!(next("10-20/5 * * * *", JAN1_2024), 10 * 60);
        assert_eq!(
            next("0 0 */10 * *", JAN1_2024 + 1),
            10 * D,
            "the 1st, 11th, 21st, 31st"
        );
    }

    #[test]
    fn cron_refuses_what_is_wrong() {
        for bad in [
            "",
            "* * * *",
            "* * * * * *",
            "60 * * * *",
            "* 24 * * *",
            "0 0 0 * *",
            "0 0 * 13 *",
            "*/0 * * * *",
            "5-1 * * * *",
            "a * * * *",
            "0 0 * * 8",
        ] {
            assert!(Cron::parse(bad).is_err(), "{bad}");
        }
        assert_eq!(
            Cron::parse("0 0 30 2 *").unwrap().next(JAN1_2024),
            None,
            "February 30"
        );
    }

    #[test]
    fn civil_dates() {
        assert_eq!(civil(0), (1970, 1, 1));
        assert_eq!(civil(19_782), (2024, 2, 29));
        assert_eq!(civil(-1), (1969, 12, 31));
    }

    #[test]
    fn failures_back_off_then_die() {
        let mut j = Job {
            payload: "1".into(),
            tries: 0,
            run_at: 0,
            lease: 99,
            dead: false,
            error: None,
        };
        let waits: Vec<u64> = (1..=4)
            .map(|_| {
                settle(&mut j, 1000, "no".into(), false);
                assert!(!j.dead && j.lease == 0);
                j.run_at - 1000
            })
            .collect();
        assert_eq!(waits, [4, 8, 16, 32]);
        settle(&mut j, 1000, "last".into(), false);
        assert!(j.dead && j.error.as_deref() == Some("last"));
        // One that no try would mend is dead on the first.
        let mut j = Job {
            tries: 0,
            dead: false,
            ..j
        };
        settle(&mut j, 1000, "bad".into(), true);
        assert!(j.dead && j.tries == 1);
        assert_eq!(backoff(40), 3600);
    }

    #[test]
    fn a_worker_runs_pushed_jobs_and_retries_failures() {
        crate::store::memory();
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async {
            let done = std::sync::Arc::new(crate::Shared::new(Vec::<String>::new()));
            let seen = done.clone();
            work("test-mail", move |n: String| {
                let seen = seen.clone();
                async move {
                    if n == "bad" {
                        return crate::error(500, "no luck");
                    }
                    if n == "boom" {
                        panic!("boom");
                    }
                    seen.lock().push(n);
                    Ok(())
                }
            });
            let q = queue("test-mail");
            q.push(&"a".to_string());
            q.push(&"bad".to_string());
            q.push(&"boom".to_string());
            q.push(&"b".to_string());
            for _ in 0..200 {
                if done.lock().len() == 2 && q.table.filter(|j| j.tries > 0).len() == 2 {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
            assert_eq!(*done.lock(), ["a", "b"]);
            let failed = q.table.filter(|j| j.tries > 0);
            assert_eq!(
                failed.len(),
                2,
                "the two that failed wait to be tried again"
            );
            assert!(
                failed
                    .iter()
                    .all(|r| r.run_at > crate::unix_now() && !r.dead)
            );
            assert!(
                failed
                    .iter()
                    .any(|r| r.error.as_deref().is_some_and(|e| e.contains("no luck")))
            );
            assert!(
                failed
                    .iter()
                    .any(|r| r.error.as_deref().is_some_and(|e| e.contains("panic")))
            );
            assert_eq!(q.pending(), 2);
            // Out of tries: dead, until put back.
            let id = failed[0].id;
            q.table.update(id, |j| j.tries = TRIES - 1);
            q.table.update(id, |j| settle(j, 0, "x".into(), false));
            assert_eq!(q.dead().len(), 1);
            assert!(q.retry(id) && !q.retry(id));
            assert_eq!(q.dead().len(), 0);
        });
        let again = std::panic::catch_unwind(|| work("test-mail", |_: String| async { Ok(()) }));
        assert!(again.is_err(), "one worker per queue");
    }

    #[test]
    fn a_host_trigger_runs_its_schedule_and_drains_the_queues() {
        crate::store::memory();
        let rt = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        let log = std::sync::Arc::new(crate::Shared::new(Vec::<String>::new()));
        let (cron_log, job_log) = (log.clone(), log.clone());
        SCHEDULED.lock().push((
            wisp_shared::cron_slug("*/5 * * * *"),
            Box::new(move || {
                let log = cron_log.clone();
                Box::pin(async move { log.lock().push("cron".into()) })
            }),
        ));
        let q = queue("test-host");
        HOSTED.lock().push((
            q,
            runner(move |n: String| {
                let log = job_log.clone();
                async move {
                    if n == "bad" {
                        return crate::error(500, "no luck");
                    }
                    log.lock().push(n);
                    Ok(())
                }
            }),
        ));
        q.push(&"a".to_string());
        q.push(&"bad".to_string());
        rt.block_on(fire("0_3_*_*_*"));
        assert_eq!(*log.lock(), ["a"], "another schedule: no cron, the jobs");
        assert_eq!(q.pending(), 1, "the failed one waits to be tried again");
        rt.block_on(fire("*~5_*_*_*_*"));
        assert_eq!(*log.lock(), ["a", "cron"]);
    }
}
