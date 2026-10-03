//! Channels: messages from one request to every other that listens, in this
//! process. A chat room, live notifications, a job's progress: a handler
//! sends to `wisp::channel("room")`, and each WebSocket or event stream
//! subscribed to it gets the message.
//!
//! In process unless a [`Relay`](crate::Relay) is set (`wisp::relay`, Redis
//! or Postgres `LISTEN` behind it): then each message goes to the app's other
//! servers too. The edge build, where every request may be its own instance,
//! has none.

use crate::{Gone, WebSocket, http};
use std::collections::BTreeMap;
use std::sync::{Arc, RwLock};
use tokio::sync::broadcast;

/// How many messages a subscriber may fall behind by before it misses some.
const BACKLOG: usize = 256;

/// The channels by name, and how many there may be before the next sweep.
struct Channels {
    /// By name, which the channels share (the relay is told it).
    all: BTreeMap<Arc<str>, broadcast::Sender<Arc<str>>>,
    sweep_at: usize,
}

/// Channels made before the first sweep: past this, and then past twice as
/// many as the last sweep kept, the ones nothing holds or listens to go.
const SWEEP_AT: usize = 1024;

static CHANNELS: RwLock<Channels> = RwLock::new(Channels {
    all: BTreeMap::new(),
    sweep_at: SWEEP_AT,
});

/// How many channels each thread keeps at hand (`NEAR`).
const NEAR_MAX: usize = 8;

/// The channels a thread used last, by name, and the slot the next one
/// goes in once all are used: the oldest.
struct Near {
    slots: Vec<(String, Channel)>,
    next: usize,
}

thread_local! {
    /// The channels this thread used last: a handler that sends to the
    /// same few names takes no lock. Each one held here is held in
    /// `CHANNELS` too (a sweep keeps what is held), so it is still the
    /// channel of its name.
    static NEAR: std::cell::RefCell<Near> =
        const { std::cell::RefCell::new(Near { slots: Vec::new(), next: 0 }) };
}

/// The channel called `name`, made on first use: every call with the same
/// name gets the same channel. One that no `Channel` or subscriber holds
/// may be forgotten (nothing could hear it anyway), so names made from
/// requests (a room per id) do not pile up.
pub fn channel(name: &str) -> Channel {
    let near = NEAR.with_borrow(|n| {
        let (_, c) = n.slots.iter().find(|(k, _)| k == name)?;
        Some(c.clone())
    });
    if let Some(c) = near {
        return c;
    }
    let c = shared(name);
    NEAR.with_borrow_mut(|n| {
        if n.slots.len() < NEAR_MAX {
            n.slots.push((name.to_owned(), c.clone()));
            return;
        }
        // The name's text goes where the oldest one's was: no allocation.
        let (k, old) = &mut n.slots[n.next];
        k.clear();
        k.push_str(name);
        *old = c.clone();
        n.next = (n.next + 1) % NEAR_MAX;
    });
    c
}

/// The channel called `name`, from `CHANNELS`.
fn shared(name: &str) -> Channel {
    let found = CHANNELS.read().unwrap_or_else(|e| e.into_inner());
    if let Some((name, tx)) = found.all.get_key_value(name) {
        return Channel(tx.clone(), name.clone());
    }
    drop(found);
    let mut c = CHANNELS.write().unwrap_or_else(|e| e.into_inner());
    if c.all.len() >= c.sweep_at && !c.all.contains_key(name) {
        c.all
            .retain(|_, tx| tx.strong_count() > 1 || tx.receiver_count() > 0);
        c.sweep_at = (2 * c.all.len()).max(SWEEP_AT);
    }
    let name: Arc<str> = c
        .all
        .get_key_value(name)
        .map_or_else(|| name.into(), |(k, _)| k.clone());
    let tx = c
        .all
        .entry(name.clone())
        .or_insert_with(|| broadcast::channel(BACKLOG).0);
    Channel(tx.clone(), name)
}

/// A channel from [`channel`]. Cloning it is cheap; clones are the same
/// channel.
#[derive(Clone)]
pub struct Channel(broadcast::Sender<Arc<str>>, Arc<str>);

impl Channel {
    /// Sends `message` to every subscriber there is now (and, with a relay,
    /// the other servers'), and returns how many here that is. With none, it goes nowhere. Each subscriber shares
    /// the one copy.
    pub fn send(&self, message: impl Into<Arc<str>>) -> usize {
        let message = message.into();
        crate::relay::publish(&self.1, &message);
        self.send_here(message)
    }

    /// [`Channel::send`] to this process's subscribers only: what the
    /// relay brings in.
    pub(crate) fn send_here(&self, message: impl Into<Arc<str>>) -> usize {
        self.0.send(message.into()).unwrap_or(0)
    }

    /// Receives what is sent from now on.
    pub fn subscribe(&self) -> Subscription {
        Subscription(self.0.subscribe())
    }

    /// How many subscribers there are.
    pub fn subscribers(&self) -> usize {
        self.0.receiver_count()
    }

    /// What is sent from now on, as server-sent events, until the client
    /// leaves: `fn get() -> Response { wisp::channel("notes").events() }`.
    /// Subscribed before it answers, so nothing sent after is missed.
    pub fn events(&self) -> crate::Response {
        let mut sub = self.subscribe();
        crate::Response::events(|out| async move {
            while let Some(message) = sub.recv().await {
                out.event(&message).await?;
            }
            Ok(())
        })
    }

    /// A WebSocket joined to the channel (see [`Channel::connect`]): a chat
    /// room is `fn get() -> Response { wisp::channel("chat").websocket() }`.
    pub fn websocket(&self) -> crate::Response {
        let me = self.clone();
        crate::Response::websocket(|ws| async move {
            me.connect(&ws).await?;
            Ok(())
        })
    }

    /// Joins `ws` to the channel until the client leaves: each text message
    /// it sends goes to the channel, and each message on the channel (its
    /// own too) goes to it; [`Channel::websocket`] answers with one.
    pub async fn connect(&self, ws: &WebSocket) -> Result<(), Gone> {
        enum Next {
            Client(Option<crate::Message>),
            Channel(Option<Arc<str>>),
        }
        let mut sub = self.subscribe();
        loop {
            // Both are safe to drop part way: neither loses a message.
            let next = http::first(async { Next::Client(ws.recv().await) }, async {
                Next::Channel(sub.recv().await)
            })
            .await;
            match next {
                Next::Client(Some(crate::Message::Text(text))) => {
                    self.send(text);
                }
                Next::Client(Some(crate::Message::Binary(_))) => {}
                Next::Channel(Some(text)) => ws.send(&*text).await?,
                Next::Client(None) | Next::Channel(None) => return Ok(()),
            }
        }
    }
}

/// What a [`Channel::subscribe`] receives.
pub struct Subscription(broadcast::Receiver<Arc<str>>);

impl Subscription {
    /// The next message. A subscriber that fell more than 256 messages
    /// behind skips to the oldest it still has. `None` once the server is
    /// stopping.
    pub async fn recv(&mut self) -> Option<Arc<str>> {
        loop {
            let next = http::first(async { Some(self.0.recv().await) }, async {
                http::stopped().await;
                None
            });
            match next.await? {
                Ok(msg) => return Some(msg),
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => return None,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_name_same_channel() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        rt.block_on(async {
            let a = channel("test-room");
            assert_eq!(a.send("nobody hears"), 0);
            let mut sub = channel("test-room").subscribe();
            let mut other = channel("test-other").subscribe();
            assert_eq!(a.subscribers(), 1);
            assert_eq!(a.send("hi"), 1);
            assert_eq!(sub.recv().await.as_deref(), Some("hi"));
            for i in 0..BACKLOG + 5 {
                a.send(i.to_string());
            }
            assert_eq!(sub.recv().await, Some("5".into()), "a slow one skips ahead");
            assert!(other.0.try_recv().is_err());
        });
    }

    #[test]
    fn channels_nothing_holds_are_forgotten() {
        let kept = channel("test-kept");
        let mut sub = channel("test-heard").subscribe();
        for i in 0..5 * SWEEP_AT {
            channel(&format!("test-room-{i}")).send("x");
        }
        let n = CHANNELS.read().unwrap().all.len();
        assert!(n <= 2 * SWEEP_AT + 8, "{n} channels");
        let _late = channel("test-kept").subscribe();
        assert_eq!(kept.send("x"), 1, "one held is still the same channel");
        assert_eq!(channel("test-heard").send("hi"), 1);
        assert_eq!(sub.0.try_recv().as_deref().ok(), Some("hi"));
    }
}
