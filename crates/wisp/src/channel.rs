//! Channels: messages from one request to every other that listens, in this
//! process. A chat room, live notifications, a job's progress: a handler
//! sends to `wisp::channel("room")`, and each WebSocket or event stream
//! subscribed to it gets the message.
//!
//! In process only: several servers of one app each have their own (put
//! Redis or Postgres `LISTEN` behind them for that), and the edge build,
//! where every request may be its own instance, has none.

use crate::{Gone, WebSocket, http};
use std::sync::Mutex;
use tokio::sync::broadcast;

/// How many messages a subscriber may fall behind by before it misses some.
const BACKLOG: usize = 256;

/// The channel called `name`, made on first use: every call with the same
/// name gets the same channel.
pub fn channel(name: &str) -> Channel {
    static ALL: Mutex<Vec<(String, broadcast::Sender<String>)>> = Mutex::new(Vec::new());
    let mut all = ALL.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((_, tx)) = all.iter().find(|(n, _)| n == name) {
        return Channel(tx.clone());
    }
    let (tx, _) = broadcast::channel(BACKLOG);
    all.push((name.to_string(), tx.clone()));
    Channel(tx)
}

/// A channel from [`channel`]. Cloning it is cheap; clones are the same
/// channel.
#[derive(Clone)]
pub struct Channel(broadcast::Sender<String>);

impl Channel {
    /// Sends `message` to every subscriber there is now, and returns how
    /// many that is. With none, it goes nowhere.
    pub fn send(&self, message: impl Into<String>) -> usize {
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

    /// Joins `ws` to the channel until the client leaves: each text message
    /// it sends goes to the channel, and each message on the channel (its
    /// own too) goes to it. A chat room is
    /// `Response::websocket(|ws| async move { wisp::channel("chat").connect(&ws).await?; Ok(()) })`.
    pub async fn connect(&self, ws: &WebSocket) -> Result<(), Gone> {
        enum Next {
            Client(Option<crate::Message>),
            Channel(Option<String>),
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
                Next::Channel(Some(text)) => ws.send(text).await?,
                Next::Client(None) | Next::Channel(None) => return Ok(()),
            }
        }
    }
}

/// What a [`Channel::subscribe`] receives.
pub struct Subscription(broadcast::Receiver<String>);

impl Subscription {
    /// The next message. A subscriber that fell more than 256 messages
    /// behind skips to the oldest it still has. `None` once the server is
    /// stopping.
    pub async fn recv(&mut self) -> Option<String> {
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
}
