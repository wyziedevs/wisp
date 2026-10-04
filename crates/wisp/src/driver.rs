//! What the Linux drivers (`uring.rs`, `epoll.rs`) share: the slab their
//! connections' entries live in, the tokens their completions and events
//! carry, the worker each thread keeps, and the `Sock` a connection holds.
//! Each driver keeps only its own receive, send and handover to tokio.
//! Static generics only: every call resolves to the driver's own code.

use std::cell::RefCell;
use std::future::poll_fn;
use std::io;
use std::marker::PhantomData;
use std::net::{Shutdown, TcpStream};
use std::ops::{Deref, DerefMut};
use std::os::fd::AsRawFd;
use std::os::fd::RawFd;
use std::task::{Context, Poll, Waker};
use std::thread::LocalKey;

/// The `u64` a completion or event carries for `slot` (a connection's
/// entry, for the ring with its receive or send) in generation `generation`
/// of the entry: one taken in the turn that closed a connection cannot reach
/// the next one in the same entry.
pub(crate) fn token(slot: usize, generation: u32) -> u64 {
    (u64::from(generation) << 32) | slot as u64
}

/// The slot and generation of a `token`.
pub(crate) fn untoken(t: u64) -> (usize, u32) {
    (t as u32 as usize, (t >> 32) as u32)
}

/// `waker` is `cx`'s from now on, cloned only when it changed.
pub(crate) fn wait(waker: &mut Option<Waker>, cx: &Context) {
    if !waker.as_ref().is_some_and(|w| w.will_wake(cx.waker())) {
        *waker = Some(cx.waker().clone());
    }
}

/// A connection's calls after `errno`, 0 for none.
pub(crate) fn result(errno: i32) -> io::Result<()> {
    match errno {
        0 => Ok(()),
        e => Err(io::Error::from_raw_os_error(e)),
    }
}

/// An entry of a `Slab`: its generation counts the connections it held.
pub(crate) trait Slot: Default {
    fn generation(&self) -> u32;
}

/// A worker's connection entries, reused through a free list. Reads as the
/// `Vec` of entries.
pub(crate) struct Slab<E> {
    conns: Vec<E>,
    free: Vec<usize>,
}

impl<E: Slot> Slab<E> {
    pub(crate) const fn new() -> Slab<E> {
        Slab {
            conns: Vec::new(),
            free: Vec::new(),
        }
    }

    /// A free entry, and the generation its next connection takes.
    pub(crate) fn open(&mut self) -> (usize, u32) {
        let id = self.free.pop().unwrap_or_else(|| {
            self.conns.push(E::default());
            self.conns.len() - 1
        });
        (id, self.conns[id].generation().wrapping_add(1))
    }

    /// Entry `id` is free for the next `open`.
    pub(crate) fn free(&mut self, id: usize) {
        self.free.push(id);
    }

    /// Some entry is in use.
    pub(crate) fn busy(&self) -> bool {
        self.conns.len() > self.free.len()
    }
}

impl<E> Deref for Slab<E> {
    type Target = Vec<E>;
    fn deref(&self) -> &Vec<E> {
        &self.conns
    }
}

impl<E> DerefMut for Slab<E> {
    fn deref_mut(&mut self) -> &mut Vec<E> {
        &mut self.conns
    }
}

/// A driver's worker, one a thread, as the shared `Sock` reaches it.
pub(crate) trait Driver: Sized + 'static {
    /// The thread's worker, set once its driver starts.
    fn local() -> &'static LocalKey<RefCell<Option<Self>>>;
    /// What a call off the worker's thread panics with (a bug, never I/O).
    const OFF: &'static str;
    /// An entry for the socket `fd`.
    fn open(&mut self, fd: RawFd) -> usize;
    /// Sends all of `buf` (or queues it) and leaves it empty.
    fn write(&mut self, id: usize, buf: &mut Vec<u8>) -> io::Result<()>;
    /// Ready once the send under way is done, with its result.
    fn flushed(&mut self, id: usize, cx: &Context) -> Poll<io::Result<()>>;
    /// The `Sock` of `id` is gone, with its socket unless tokio has that.
    fn close(&mut self, id: usize, stream: Option<TcpStream>);
}

/// Runs `f` on this thread's worker of driver `D`.
pub(crate) fn with<D: Driver, R>(f: impl FnOnce(&mut D) -> R) -> R {
    D::local().with_borrow_mut(|w| f(w.as_mut().expect(D::OFF)))
}

/// A connection on its worker's driver. Like the future that holds it, it
/// stays on the worker's thread.
pub(crate) struct Sock<D: Driver> {
    pub(crate) id: usize,
    pub(crate) stream: Option<TcpStream>,
    /// Send like the socket: the worker is named, never held.
    driver: PhantomData<fn() -> D>,
}

impl<D: Driver> Sock<D> {
    pub(crate) fn new(stream: TcpStream) -> Sock<D> {
        let id = with(|w: &mut D| w.open(stream.as_raw_fd()));
        Sock {
            id,
            stream: Some(stream),
            driver: PhantomData,
        }
    }

    /// Sends all of `buf` and leaves it empty. What the socket has no room
    /// for goes on without the caller; a failure then shows in the next call.
    pub(crate) async fn write(&mut self, buf: &mut Vec<u8>) -> io::Result<()> {
        self.flush().await?;
        with(|w: &mut D| w.write(self.id, buf))
    }

    /// Waits for the send under way.
    pub(crate) async fn flush(&self) -> io::Result<()> {
        poll_fn(|cx| with(|w: &mut D| w.flushed(self.id, cx))).await
    }

    /// Ends the sending side, once all of it has been sent.
    pub(crate) async fn shutdown(&mut self) {
        if self.flush().await.is_ok()
            && let Some(s) = &self.stream
        {
            let _ = s.shutdown(Shutdown::Write);
        }
    }
}

impl<D: Driver> Drop for Sock<D> {
    fn drop(&mut self) {
        let _ = D::local().try_with(|w| {
            if let Ok(mut w) = w.try_borrow_mut()
                && let Some(w) = w.as_mut()
            {
                w.close(self.id, self.stream.take());
            }
        });
    }
}
