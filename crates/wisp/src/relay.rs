//! Relay: channels across several servers of one app. A [`Relay`] carries
//! what [`channel`](crate::channel)s send to the other instances, over
//! Redis, Postgres `LISTEN`, NATS or anything that moves text:
//!
//! ```ignore
//! // src/hooks.rs
//! struct Redis(redis::Client);
//!
//! impl wisp::Relay for Redis {
//!     fn publish(&self, channel: &str, message: &str) {
//!         // PUBLISH wisp:{channel} {message}
//!     }
//!     fn subscribe(&self, deliver: wisp::Deliver) {
//!         std::thread::spawn(move || {
//!             // PSUBSCRIBE wisp:*; for each: deliver(channel, message)
//!         });
//!     }
//! }
//!
//! async fn init() -> Result {
//!     wisp::relay(Redis(redis::Client::open(wisp::env("REDIS_URL").or_status(500)?)?));
//!     Ok(())
//! }
//! ```
//!
//! With none set, nothing changes and nothing is paid: `send` looks at
//! one empty cell. Each message carries the id of the instance that sent
//! it, and an instance drops its own when the relay brings them back, so a
//! broker that echoes (Redis does) is fine. Delivery is as good as the
//! relay's: a message sent while the broker is down reaches this
//! instance's own listeners only.

use std::sync::OnceLock;

/// What a [`Relay`] calls for each message from the broker: the channel's
/// name and the text as `publish` was given it.
pub type Deliver = Box<dyn Fn(&str, &str) + Send + Sync>;

/// A way to carry channel messages between instances of the app. Both
/// calls come from any thread and must not block for long: `publish` is
/// called by `Channel::send`.
pub trait Relay: Send + Sync + 'static {
    /// Sends `message` on `channel` to the other instances. A failure is
    /// the relay's to report; the sender goes on.
    fn publish(&self, channel: &str, message: &str);
    /// Starts receiving, once, when the relay is set: for each message
    /// that arrives (the app's own included), call `deliver`. Start your
    /// own thread or task here and return.
    fn subscribe(&self, deliver: Deliver);
}

struct Set {
    relay: &'static dyn Relay,
    /// This instance's id, which prefixes what it publishes.
    id: String,
}

static RELAY: OnceLock<Set> = OnceLock::new();

/// Carries every channel's messages to the other instances through
/// `relay`, from now on. Call it once, in `init`; a second call is ignored.
pub fn relay(relay: impl Relay) {
    let relay: &'static dyn Relay = Box::leak(Box::new(relay));
    let id = crate::hex(&crate::sign::random::<8>());
    let mine = id.clone();
    if RELAY.set(Set { relay, id }).is_err() {
        return;
    }
    relay.subscribe(Box::new(move |name, message| {
        let Some((from, text)) = message.split_once(' ') else {
            return;
        };
        if from != mine {
            crate::channel::channel(name).send_here(text);
        }
    }));
}

/// Hands a message just sent on `channel` to the relay, if there is one.
pub(crate) fn publish(channel: &str, message: &str) {
    if let Some(s) = RELAY.get() {
        s.relay.publish(channel, &format!("{} {message}", s.id));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// A broker in memory that echoes every publish back, as Redis does.
    struct Loop(Mutex<Option<Deliver>>, Mutex<Vec<(String, String)>>);

    impl Relay for &'static Loop {
        fn publish(&self, channel: &str, message: &str) {
            self.1.lock().unwrap().push((channel.into(), message.into()));
            if let Some(d) = &*self.0.lock().unwrap() {
                d(channel, message);
            }
        }
        fn subscribe(&self, deliver: Deliver) {
            *self.0.lock().unwrap() = Some(deliver);
        }
    }

    #[test]
    fn sends_go_out_and_others_come_in() {
        let broker: &'static Loop = Box::leak(Box::new(Loop(Mutex::new(None), Mutex::new(vec![]))));
        relay(broker);
        relay(broker); // ignored
        let rt = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        rt.block_on(async {
            let mut sub = crate::channel("relay-room").subscribe();
            assert_eq!(crate::channel("relay-room").send("hi"), 1);
            assert_eq!(sub.recv().await.as_deref(), Some("hi"));
            // The echo of its own is dropped: one copy, not two.
            let sent: Vec<_> = (broker.1.lock().unwrap().iter()).filter(|(c, _)| c == "relay-room").cloned().collect();
            assert_eq!(sent.len(), 1);
            let (name, wire) = &sent[0];
            assert_eq!(name, "relay-room");
            assert!(wire.ends_with(" hi"));
            // Another instance's message reaches this one's listeners, and
            // is not sent on again.
            let d = broker.0.lock().unwrap();
            d.as_ref().unwrap()("relay-room", "0000000000000000 from afar");
            d.as_ref().unwrap()("relay-room", "garbage");
            drop(d);
            assert_eq!(sub.recv().await.as_deref(), Some("from afar"));
            assert_eq!(broker.1.lock().unwrap().iter().filter(|(c, _)| c == "relay-room").count(), 1);
        });
        assert!(RELAY.get().is_some());
    }
}
