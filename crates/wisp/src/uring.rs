//! The Linux server's sockets: each worker serves a listener of its own
//! through an io_uring of its own, inside the tokio runtime it already has.
//!
//! One `io_uring_enter` per turn of a worker's driver submits every send its
//! connections queued since the last turn and runs the completions that came
//! in meanwhile (`DEFER_TASKRUN`: the kernel does that work in the call, not
//! by interrupting the thread). A connection's receive stays armed for its
//! whole life (multishot, into buffers the ring lends and gets back at once),
//! and accepting is one multishot request per worker. The kernel spreads new
//! connections over the workers' listeners (`SO_REUSEPORT`), so no thread
//! hands them out.
//!
//! The ring lives inside tokio rather than beside it: its eventfd is one more
//! thing tokio's epoll waits on, and completions wake the connections' tasks
//! like any other I/O, so handlers await timers, channels and database
//! drivers as before. A WebSocket's socket is handed over to tokio.
//!
//! What works is not guessed: at start, once, a throwaway ring receives and
//! sends through the code the workers run, lending its buffers through a
//! buffer ring. Where that does not work (before Linux 6.1, a seccomp
//! profile that refuses io_uring, the `io_uring_disabled` sysctl, some 6.8
//! builds that refuse buffer rings), and with `WISP_IO=epoll`, the workers
//! run the same way on an epoll each instead (`epoll.rs`): buffers provided
//! by `PROVIDE_BUFFERS` instead of a ring measured slower than it. One line
//! on stderr says which, and why not better.
//!
//! With `epoll.rs`, the `unsafe` of a native build: the ring's setup and the
//! memory it shares with the kernel, and the socket calls std has no word
//! for. Each block says why it holds.
#![allow(unsafe_code)]

use crate::http;
use std::cell::RefCell;
use std::future::poll_fn;
use std::io;
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream};
use std::os::fd::{AsRawFd, FromRawFd, IntoRawFd, OwnedFd, RawFd};
use std::os::unix::net::UnixStream;
use std::sync::atomic::{AtomicU16, AtomicU32, Ordering};
use std::task::{Context, Poll, Waker};
use std::time::Duration;
use tokio::io::Interest;
use tokio::io::unix::AsyncFd;
use tokio::time::Instant;

/// Submission queue entries: room for every send and receive a turn queues.
const SQ_ENTRIES: u32 = 512;
/// Completion queue entries. Any more wait in the kernel (`FEAT_NODROP`).
const CQ_ENTRIES: u32 = 4096;
/// Buffers the kernel receives into, each given back once its bytes are
/// copied out: one per connection that received something in a turn. When
/// they run out a receive stops (`ENOBUFS`) and is rearmed. They go to the
/// kernel through a buffer ring.
const BUFS: usize = 256;
const BUF_SIZE: usize = 4096;
/// Received bytes a connection has not read, past which receiving pauses
/// until it does: the rest waits in the socket, and TCP slows the client.
const INBOX_LIMIT: usize = http::KEEP_CAPACITY;

// The kernel's ABI (linux/io_uring.h): the part used here.
const SETUP_CQSIZE: u32 = 1 << 3;
const SETUP_CLAMP: u32 = 1 << 4;
const SETUP_R_DISABLED: u32 = 1 << 6;
const SETUP_SUBMIT_ALL: u32 = 1 << 7;
const SETUP_COOP_TASKRUN: u32 = 1 << 8;
const SETUP_TASKRUN_FLAG: u32 = 1 << 9;
const SETUP_SINGLE_ISSUER: u32 = 1 << 12;
const SETUP_DEFER_TASKRUN: u32 = 1 << 13;
const FEAT_SINGLE_MMAP: u32 = 1 << 0;
const FEAT_NODROP: u32 = 1 << 1;
const FEAT_FAST_POLL: u32 = 1 << 5;
const OFF_SQ_RING: i64 = 0;
const OFF_SQES: i64 = 0x1000_0000;
const ENTER_GETEVENTS: u32 = 1 << 0;
const REGISTER_EVENTFD: u32 = 4;
const REGISTER_ENABLE_RINGS: u32 = 12;
const REGISTER_PBUF_RING: u32 = 22;
/// The kernel makes the buffer ring's memory (`IOU_PBUF_RING_MMAP`), at this offset of the ring's fd (group 0).
const PBUF_RING_MMAP: u16 = 1;
const OFF_PBUF_RING: i64 = 0x8000_0000;
const SQ_CQ_OVERFLOW: u32 = 1 << 1;
const SQ_TASKRUN: u32 = 1 << 2;
const OP_ACCEPT: u8 = 13;
const OP_ASYNC_CANCEL: u8 = 14;
const OP_CLOSE: u8 = 19;
const OP_SEND: u8 = 26;
const OP_RECV: u8 = 27;
const SQE_BUFFER_SELECT: u8 = 1 << 5;
const ACCEPT_MULTISHOT: u16 = 1 << 0;
const RECV_MULTISHOT: u16 = 1 << 1;
const CQE_F_BUFFER: u32 = 1 << 0;
const CQE_F_MORE: u32 = 1 << 1;

/// The `user_data` of the worker's accepts, and of operations whose
/// completions say nothing worth knowing (cancels, closes). A connection's
/// is its entry, generation and whether it is the send (see `op`).
const ACCEPT: u64 = u64::MAX - 1;
const IGNORE: u64 = u64::MAX;

/// io_uring_params.
#[repr(C)]
#[derive(Default)]
#[allow(dead_code)] // the kernel's fields, some only for it
struct Params {
    sq_entries: u32,
    cq_entries: u32,
    flags: u32,
    sq_thread_cpu: u32,
    sq_thread_idle: u32,
    features: u32,
    wq_fd: u32,
    resv: [u32; 3],
    sq_off: SqOffsets,
    cq_off: CqOffsets,
}

#[repr(C)]
#[derive(Default)]
#[allow(dead_code)]
struct SqOffsets {
    head: u32,
    tail: u32,
    ring_mask: u32,
    ring_entries: u32,
    flags: u32,
    dropped: u32,
    array: u32,
    resv1: u32,
    user_addr: u64,
}

#[repr(C)]
#[derive(Default)]
#[allow(dead_code)]
struct CqOffsets {
    head: u32,
    tail: u32,
    ring_mask: u32,
    ring_entries: u32,
    overflow: u32,
    cqes: u32,
    flags: u32,
    resv1: u32,
    user_addr: u64,
}

/// A submission (io_uring_sqe), as far as these operations use it.
#[repr(C)]
#[derive(Clone, Copy, Default)]
#[allow(dead_code)] // read by the kernel
struct Sqe {
    opcode: u8,
    flags: u8,
    ioprio: u16,
    fd: i32,
    off: u64,
    addr: u64,
    len: u32,
    op_flags: u32,
    user_data: u64,
    buf_group: u16,
    personality: u16,
    file_index: u32,
    addr3: u64,
    pad: u64,
}

/// A completion (io_uring_cqe).
#[repr(C)]
#[derive(Clone, Copy)]
struct Cqe {
    user_data: u64,
    res: i32,
    flags: u32,
}

/// io_uring_buf_reg: where the buffer ring is.
#[repr(C)]
#[allow(dead_code)] // read by the kernel
struct BufReg {
    ring_addr: u64,
    ring_entries: u32,
    bgid: u16,
    flags: u16,
    resv: [u64; 3],
}

/// The `user_data` of connection `id`'s receive or send, in generation
/// `generation` of its entry: a cancel queued for an old connection cannot
/// reach the next one in the same entry.
fn op(id: usize, generation: u32, send: bool) -> u64 {
    (u64::from(generation) << 32) | ((id as u64) << 1) | u64::from(send)
}

/// The descriptor a call returned, or its error.
pub(crate) fn owned(r: impl Into<i64>) -> io::Result<OwnedFd> {
    let r = r.into();
    if r < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: a descriptor the kernel just made for us; nothing else owns it.
    Ok(unsafe { OwnedFd::from_raw_fd(r as RawFd) })
}

/// io_uring_register with `arg` (`nr` of them).
fn register<T>(ring: &OwnedFd, opcode: u32, arg: Option<&T>, nr: u32) -> io::Result<()> {
    let arg: *const T = arg.map_or(std::ptr::null(), |a| a);
    // SAFETY: `arg` is null or what `opcode` reads, alive for the call.
    let r = unsafe {
        libc::syscall(
            libc::SYS_io_uring_register,
            libc::c_long::from(ring.as_raw_fd()),
            libc::c_long::from(opcode),
            arg,
            libc::c_long::from(nr),
        )
    };
    if r < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// Memory mapped for the ring's life, shared with the kernel.
struct Map(*mut u8, usize);

impl Map {
    /// `len` bytes of the ring `fd` at `offset`, or of new memory (`fd` -1).
    fn new(fd: RawFd, len: usize, offset: i64) -> io::Result<Map> {
        let flags = match fd {
            -1 => libc::MAP_PRIVATE | libc::MAP_ANONYMOUS,
            _ => libc::MAP_SHARED | libc::MAP_POPULATE,
        };
        let prot = libc::PROT_READ | libc::PROT_WRITE;
        // SAFETY: a new mapping at an address the kernel picks, which nothing
        // else refers to.
        let p = unsafe { libc::mmap(std::ptr::null_mut(), len, prot, flags, fd, offset) };
        if p == libc::MAP_FAILED {
            return Err(io::Error::last_os_error());
        }
        Ok(Map(p.cast(), len))
    }

    /// The 32-bit word at byte `at`, which the kernel reads or writes too.
    fn word(&self, at: u32) -> &AtomicU32 {
        debug_assert!(at as usize + 4 <= self.1 && at.is_multiple_of(4));
        // SAFETY: an aligned word inside the mapping (the kernel's own
        // offsets), which lives as long as `self`; the kernel reads and
        // writes it atomically too.
        unsafe { &*self.0.add(at as usize).cast::<AtomicU32>() }
    }
}

impl Drop for Map {
    fn drop(&mut self) {
        // SAFETY: our own mapping, which nothing uses any more.
        unsafe { libc::munmap(self.0.cast(), self.1) };
    }
}

/// One worker's io_uring: its queues, and the buffers it receives into.
pub(crate) struct Ring {
    fd: OwnedFd,
    eventfd: Option<OwnedFd>,
    rings: Map,
    sqes: Map,
    /// The buffer ring the kernel takes buffers from: `BUFS` entries of 16
    /// bytes.
    pring: Map,
    /// The buffers themselves.
    bufs: Map,
    sq_head: u32,
    sq_tail: u32,
    sq_flags: u32,
    sq_mask: u32,
    sq_len: u32,
    cq_head: u32,
    cq_tail: u32,
    cq_mask: u32,
    cqes: u32,
    /// Our ends of the queues: the next submission's slot, the next
    /// completion to read, the next buffer ring entry.
    tail: u32,
    head: u32,
    buf_tail: u16,
}

// SAFETY: the pointers are into mappings the ring owns. It is made on the
// main thread and moved to its worker before any use; after that only the
// worker touches it.
unsafe impl Send for Ring {}

/// The ring `fd` takes its receive buffers from. Memory of ours, registered,
/// where the kernel allows it; else memory the kernel makes and we map (Linux
/// 6.4 on), which some kernels insist on. Some refuse both.
fn buffer_ring(fd: &OwnedFd) -> io::Result<Map> {
    let mut reg = BufReg {
        ring_addr: 0,
        ring_entries: BUFS as u32,
        bgid: 0,
        flags: 0,
        resv: [0; 3],
    };
    let ours = Map::new(-1, BUFS * 16, 0)?;
    reg.ring_addr = ours.0 as u64;
    if register(fd, REGISTER_PBUF_RING, Some(&reg), 1).is_ok() {
        return Ok(ours);
    }
    reg.ring_addr = 0;
    reg.flags = PBUF_RING_MMAP;
    register(fd, REGISTER_PBUF_RING, Some(&reg), 1)
        .map_err(|e| step("registering the buffer ring", e))?;
    Map::new(fd.as_raw_fd(), BUFS * 16, OFF_PBUF_RING)
        .map_err(|e| step("mapping the buffer ring", e))
}

/// `e`, saying which step of making a ring it came from.
fn step(what: &str, e: io::Error) -> io::Error {
    io::Error::new(e.kind(), format!("{what}: {e}"))
}

impl Ring {
    /// A ring for one worker. It stays disabled until `serve` enables it on
    /// the worker's thread, which then is the only one that submits. Its
    /// buffers go to the kernel through a buffer ring: an error where refused.
    fn new() -> io::Result<Ring> {
        let mut p = Params {
            cq_entries: CQ_ENTRIES,
            flags: SETUP_SINGLE_ISSUER
                | SETUP_DEFER_TASKRUN
                | SETUP_COOP_TASKRUN
                | SETUP_TASKRUN_FLAG
                | SETUP_SUBMIT_ALL
                | SETUP_CQSIZE
                | SETUP_CLAMP
                | SETUP_R_DISABLED,
            ..Params::default()
        };
        // SAFETY: `p` is an io_uring_params for the kernel to fill in.
        let fd = owned(unsafe {
            libc::syscall(
                libc::SYS_io_uring_setup,
                libc::c_long::from(SQ_ENTRIES),
                &raw mut p,
            )
        })
        .map_err(|e| step("io_uring_setup", e))?;
        let need = FEAT_SINGLE_MMAP | FEAT_NODROP | FEAT_FAST_POLL;
        if p.features & need != need {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "io_uring_setup: this kernel's io_uring lacks features Wisp needs",
            ));
        }
        let (sq, cq) = (&p.sq_off, &p.cq_off);
        let len = (sq.array + p.sq_entries * 4).max(cq.cqes + p.cq_entries * 16);
        let rings = Map::new(fd.as_raw_fd(), len as usize, OFF_SQ_RING)?;
        let sqes = Map::new(
            fd.as_raw_fd(),
            p.sq_entries as usize * size_of::<Sqe>(),
            OFF_SQES,
        )?;
        // Submission slot i is always entry i, so the index array is set once.
        for i in 0..p.sq_entries {
            rings.word(sq.array + 4 * i).store(i, Ordering::Relaxed);
        }
        // SAFETY: eventfd(2), no pointers.
        let eventfd = owned(unsafe { libc::eventfd(0, libc::EFD_CLOEXEC | libc::EFD_NONBLOCK) })?;
        register(&fd, REGISTER_EVENTFD, Some(&eventfd.as_raw_fd()), 1)
            .map_err(|e| step("registering the eventfd", e))?;
        let bufs = Map::new(-1, BUFS * BUF_SIZE, 0)?;
        let pring = buffer_ring(&fd)?;
        let mut ring = Ring {
            sq_head: sq.head,
            sq_tail: sq.tail,
            sq_flags: sq.flags,
            sq_mask: rings.word(sq.ring_mask).load(Ordering::Relaxed),
            sq_len: rings.word(sq.ring_entries).load(Ordering::Relaxed),
            cq_head: cq.head,
            cq_tail: cq.tail,
            cq_mask: rings.word(cq.ring_mask).load(Ordering::Relaxed),
            cqes: cq.cqes,
            tail: rings.word(sq.tail).load(Ordering::Relaxed),
            head: rings.word(cq.head).load(Ordering::Relaxed),
            buf_tail: 0,
            fd,
            eventfd: Some(eventfd),
            rings,
            sqes,
            pring,
            bufs,
        };
        // Given and published before the ring is enabled: all there ahead
        // of any receive.
        for bid in 0..BUFS as u16 {
            ring.give(bid);
        }
        ring.publish_bufs();
        Ok(ring)
    }

    /// Queues `sqe`: false if the queue is full, until the next `enter`.
    fn push(&mut self, sqe: Sqe) -> bool {
        if self.queued() >= self.sq_len {
            return false;
        }
        let at = self
            .sqes
            .0
            .cast::<Sqe>()
            .wrapping_add((self.tail & self.sq_mask) as usize);
        // SAFETY: a slot of the SQE array behind the kernel's head (it has
        // read it, or never used it), which it reads again only once `tail`
        // is published past it.
        unsafe { at.write(sqe) };
        self.tail = self.tail.wrapping_add(1);
        true
    }

    /// Submissions queued that the kernel has not taken.
    fn queued(&self) -> u32 {
        let head = self.rings.word(self.sq_head).load(Ordering::Acquire);
        self.tail.wrapping_sub(head)
    }

    /// Submits what is queued and, with `events`, runs the completion work
    /// that came in, so that its completions are in the queue. Receives
    /// take buffers only in here, so the buffers given back are published
    /// first.
    fn enter(&mut self, events: bool) -> io::Result<()> {
        self.publish_bufs();
        self.rings
            .word(self.sq_tail)
            .store(self.tail, Ordering::Release);
        // Exactly what is queued: with more the kernel would skip `events`.
        let n = self.queued();
        let flags = if events { ENTER_GETEVENTS } else { 0 };
        loop {
            // SAFETY: io_uring_enter(2) on our ring, without a signal mask.
            let r = unsafe {
                libc::syscall(
                    libc::SYS_io_uring_enter,
                    libc::c_long::from(self.fd.as_raw_fd()),
                    libc::c_long::from(n),
                    0 as libc::c_long,
                    libc::c_long::from(flags),
                    std::ptr::null::<libc::c_void>(),
                    0 as libc::c_long,
                )
            };
            if r >= 0 {
                return Ok(());
            }
            let e = io::Error::last_os_error();
            if e.kind() != io::ErrorKind::Interrupted {
                return Err(e);
            }
        }
    }

    /// The next completion, if one is in.
    fn next(&mut self) -> Option<Cqe> {
        if self.head == self.rings.word(self.cq_tail).load(Ordering::Acquire) {
            return None;
        }
        let at = self
            .rings
            .0
            .wrapping_add(self.cqes as usize + (self.head & self.cq_mask) as usize * 16);
        // SAFETY: a completion the kernel published (it is before the tail it
        // released), which it does not overwrite until our head passes it.
        let cqe = unsafe { at.cast::<Cqe>().read() };
        self.head = self.head.wrapping_add(1);
        self.rings
            .word(self.cq_head)
            .store(self.head, Ordering::Release);
        Some(cqe)
    }

    /// Whether completions did not fit in the queue and wait in the kernel.
    fn overflowed(&self) -> bool {
        self.rings.word(self.sq_flags).load(Ordering::Relaxed) & SQ_CQ_OVERFLOW != 0
    }

    /// Whether completion work waits for the next `enter`. A call runs only
    /// so much of it (Linux 6.13 on), and the eventfd tells of new work, not
    /// of what a call left.
    fn behind(&self) -> bool {
        self.rings.word(self.sq_flags).load(Ordering::Relaxed) & SQ_TASKRUN != 0
    }

    /// The first `len` bytes of buffer `bid`, which the kernel filled.
    fn buf(&self, bid: u16, len: usize) -> &[u8] {
        let at = (bid as usize % BUFS) * BUF_SIZE;
        // SAFETY: a buffer inside the mapping. The kernel wrote it before
        // posting the completion that named it, and writes it again only
        // once `give` hands it back.
        unsafe { std::slice::from_raw_parts(self.bufs.0.add(at), len.min(BUF_SIZE)) }
    }

    /// Hands buffer `bid` back to the kernel, which sees it from the next
    /// `enter` on.
    fn give(&mut self, bid: u16) {
        let entry = self
            .pring
            .0
            .wrapping_add(16 * (self.buf_tail as usize % BUFS));
        let addr = self.bufs.0.wrapping_add(bid as usize * BUF_SIZE);
        // SAFETY: an entry of the buffer ring that the kernel has consumed
        // (or never used) and reads again only once the published tail
        // passes it. Its last two bytes are left alone: in the first entry
        // they are the tail.
        unsafe {
            entry.cast::<u64>().write(addr as u64);
            entry.add(8).cast::<u32>().write(BUF_SIZE as u32);
            entry.add(12).cast::<u16>().write(bid);
        }
        self.buf_tail = self.buf_tail.wrapping_add(1);
    }

    fn publish_bufs(&self) {
        // SAFETY: the buffer ring's tail, bytes 14..16 of its first entry,
        // which the kernel reads atomically.
        let tail = unsafe { &*self.pring.0.add(14).cast::<AtomicU16>() };
        tail.store(self.buf_tail, Ordering::Release);
    }
}

/// A connection as its worker keeps it: in an entry that outlives the
/// `Sock` for as long as the kernel may still use the socket or its buffers.
#[derive(Default)]
struct Entry {
    fd: RawFd,
    generation: u32,
    /// Received, not yet read by the connection.
    inbox: Vec<u8>,
    /// The receive is armed: its last completion (no `F_MORE`) is not in.
    armed: bool,
    /// A cancel of the receive is queued.
    cancelling: bool,
    /// The receive stopped with a full inbox: rearmed once it is read.
    paused: bool,
    /// The socket is going to tokio: no rearming.
    detached: bool,
    /// Nothing will come after the inbox: 0 at the end of the stream, or
    /// the error.
    ended: Option<i32>,
    /// The response being sent, or (empty) the buffer the next one swaps in.
    out: Vec<u8>,
    sent: usize,
    sending: bool,
    /// When the send last made progress, in `http::seconds()`.
    since: u64,
    timed_out: bool,
    /// A send failed with this error: every later call fails.
    failed: i32,
    waker: Option<Waker>,
    /// The connection's task waits for something received, or for the send.
    wants_recv: bool,
    wants_send: bool,
    /// The `Sock` is gone. `close` is the socket to close once its send is
    /// done (none when tokio has it); the entry is free once its receive is
    /// disarmed too.
    dropped: bool,
    close: Option<RawFd>,
}

impl Entry {
    fn wait(&mut self, cx: &Context) {
        if !self.waker.as_ref().is_some_and(|w| w.will_wake(cx.waker())) {
            self.waker = Some(cx.waker().clone());
        }
    }

    fn result(errno: i32) -> io::Result<()> {
        match errno {
            0 => Ok(()),
            e => Err(io::Error::from_raw_os_error(e)),
        }
    }
}

/// A worker's ring and connections.
struct Worker {
    ring: Ring,
    conns: Vec<Entry>,
    free: Vec<usize>,
    /// The driver task's waker, and whether it was woken since it last ran:
    /// a connection that queues something wakes it, once.
    driver: Option<Waker>,
    woken: bool,
    /// Connections with a send under way, and the second those were last
    /// checked for stalls.
    sending: usize,
    checked: u64,
    listener: RawFd,
    /// Sockets accepted in this turn. `accepting`: the multishot accept is
    /// armed; when it ends, `accept_error` says why.
    accepted: Vec<RawFd>,
    accepting: bool,
    accept_error: i32,
    /// Submissions the queue had no room for, first in, submitted next turn.
    backlog: Vec<Sqe>,
}

thread_local! {
    /// This thread's worker, set once its driver starts.
    static WORKER: RefCell<Option<Worker>> = const { RefCell::new(None) };
}

fn with<R>(f: impl FnOnce(&mut Worker) -> R) -> R {
    WORKER.with_borrow_mut(|w| f(w.as_mut().expect("a ring socket used off its worker")))
}

impl Worker {
    fn queue(&mut self, sqe: Sqe) {
        // Full, which a turn's usual sends do not fill: submitting makes room.
        // A kernel too busy to take them (or a backlog already waiting, which
        // keeps the order) leaves the rest to the next turn.
        while !self.backlog.is_empty() || !self.ring.push(sqe) {
            if !self.backlog.is_empty() || self.ring.enter(false).is_err() {
                self.backlog.push(sqe);
                break;
            }
        }
        if !self.woken {
            self.woken = true;
            if let Some(w) = &self.driver {
                w.wake_by_ref();
            }
        }
    }

    fn new(ring: Ring, listener: RawFd) -> Worker {
        Worker {
            ring,
            conns: Vec::new(),
            free: Vec::new(),
            driver: None,
            woken: true,
            sending: 0,
            checked: 0,
            listener,
            accepted: Vec::new(),
            accepting: false,
            accept_error: 0,
            backlog: Vec::new(),
        }
    }

    /// One io_uring_enter: submits what is queued, runs what came in, and
    /// hands the completions out. An error is the whole ring's.
    fn turn(&mut self) -> io::Result<()> {
        let taken = self
            .backlog
            .iter()
            .take_while(|&&sqe| self.ring.push(sqe))
            .count();
        self.backlog.drain(..taken);
        let mut entered = self.ring.enter(true);
        loop {
            // Busy: completions wait in the kernel for room, which reading makes.
            if let Err(e) = entered
                && !matches!(e.raw_os_error(), Some(libc::EBUSY | libc::EAGAIN))
            {
                return Err(e);
            }
            while let Some(c) = self.ring.next() {
                self.complete(c);
            }
            if !self.ring.overflowed() {
                break;
            }
            entered = self.ring.enter(true);
        }
        if self.sending > 0 && http::seconds() != self.checked {
            self.sweep();
        }
        Ok(())
    }

    fn complete(&mut self, c: Cqe) {
        match c.user_data {
            IGNORE => {}
            ACCEPT => {
                if c.res >= 0 {
                    self.accepted.push(c.res);
                }
                if c.flags & CQE_F_MORE == 0 {
                    self.accepting = false;
                    self.accept_error = (-c.res).max(0);
                }
            }
            ud => {
                let id = ((ud as u32) >> 1) as usize;
                debug_assert_eq!(self.conns[id].generation, (ud >> 32) as u32);
                if ud & 1 == 0 {
                    self.received(id, c);
                } else {
                    self.sent(id, c.res);
                }
            }
        }
    }

    fn received(&mut self, id: usize, c: Cqe) {
        let e = &mut self.conns[id];
        if c.flags & CQE_F_BUFFER != 0 {
            let bid = (c.flags >> 16) as u16;
            if c.res > 0 && !e.dropped {
                e.inbox
                    .extend_from_slice(self.ring.buf(bid, c.res as usize));
            }
            self.ring.give(bid);
        }
        if c.flags & CQE_F_MORE == 0 {
            e.armed = false;
            e.cancelling = false;
            match -c.res {
                0 => e.ended = Some(0),
                // Stopped, but not for good: rearmed (or paused) below.
                n if n < 0 || n == libc::ECANCELED || n == libc::ENOBUFS => {}
                errno => e.ended = Some(errno),
            }
            if e.dropped {
                return self.release(id);
            }
            if e.ended.is_none() && !e.detached {
                if e.inbox.len() >= INBOX_LIMIT {
                    e.paused = true;
                } else {
                    self.arm(id);
                }
            }
        } else if e.inbox.len() >= INBOX_LIMIT && !e.cancelling {
            self.cancel(id, false);
        }
        let e = &mut self.conns[id];
        if e.wants_recv {
            e.wants_recv = false;
            if let Some(w) = &e.waker {
                w.wake_by_ref();
            }
        }
    }

    fn sent(&mut self, id: usize, res: i32) {
        let e = &mut self.conns[id];
        if res > 0 && !e.timed_out {
            e.sent += res as usize;
            if e.sent < e.out.len() {
                // The socket took part: the rest goes when it has room.
                e.since = http::seconds();
                return self.push_send(id);
            }
        } else {
            e.failed = match res {
                _ if e.timed_out => libc::ETIMEDOUT,
                0 => libc::EPIPE,
                r => -r,
            };
        }
        e.sending = false;
        e.timed_out = false;
        e.out.clear();
        if e.out.capacity() > http::KEEP_CAPACITY {
            e.out.shrink_to(http::KEEP_CAPACITY);
        }
        self.sending -= 1;
        // A failure is for a task waiting to receive too: its `read` fails.
        if e.wants_send || (e.failed != 0 && e.wants_recv) {
            (e.wants_send, e.wants_recv) = (false, false);
            if let Some(w) = &e.waker {
                w.wake_by_ref();
            }
        }
        if e.dropped {
            self.release(id);
        }
    }

    /// Cancels the sends that made no progress for `WRITE_TIMEOUT`: their
    /// clients stopped taking the response, and are dropped as the tokio
    /// path drops them.
    fn sweep(&mut self) {
        let now = http::seconds();
        self.checked = now;
        for id in 0..self.conns.len() {
            let e = &mut self.conns[id];
            if e.sending && !e.timed_out && now >= e.since + http::WRITE_TIMEOUT.as_secs() {
                e.timed_out = true;
                self.cancel(id, true);
            }
        }
    }

    fn accept(&mut self) {
        (self.accepting, self.accept_error) = (true, 0);
        self.queue(Sqe {
            opcode: OP_ACCEPT,
            fd: self.listener,
            ioprio: ACCEPT_MULTISHOT,
            op_flags: libc::SOCK_CLOEXEC as u32,
            user_data: ACCEPT,
            ..Sqe::default()
        });
    }

    /// An entry for the socket `fd`, receiving.
    fn open(&mut self, fd: RawFd) -> usize {
        let id = self.free.pop().unwrap_or_else(|| {
            self.conns.push(Entry::default());
            self.conns.len() - 1
        });
        let e = &mut self.conns[id];
        let (mut inbox, mut out) = (std::mem::take(&mut e.inbox), std::mem::take(&mut e.out));
        inbox.clear();
        out.clear();
        *e = Entry {
            fd,
            generation: e.generation.wrapping_add(1),
            inbox,
            out,
            ..Entry::default()
        };
        self.arm(id);
        id
    }

    fn arm(&mut self, id: usize) {
        let e = &mut self.conns[id];
        e.armed = true;
        let sqe = Sqe {
            opcode: OP_RECV,
            flags: SQE_BUFFER_SELECT,
            ioprio: RECV_MULTISHOT,
            fd: e.fd,
            user_data: op(id, e.generation, false),
            ..Sqe::default()
        };
        self.queue(sqe);
    }

    fn push_send(&mut self, id: usize) {
        let e = &self.conns[id];
        let rest = &e.out[e.sent..];
        let sqe = Sqe {
            opcode: OP_SEND,
            fd: e.fd,
            addr: rest.as_ptr() as u64,
            len: rest.len().min(u32::MAX as usize) as u32,
            op_flags: libc::MSG_NOSIGNAL as u32,
            user_data: op(id, e.generation, true),
            ..Sqe::default()
        };
        self.queue(sqe);
    }

    fn cancel(&mut self, id: usize, send: bool) {
        let e = &mut self.conns[id];
        e.cancelling |= !send;
        let sqe = Sqe {
            opcode: OP_ASYNC_CANCEL,
            fd: -1,
            addr: op(id, e.generation, send),
            user_data: IGNORE,
            ..Sqe::default()
        };
        self.queue(sqe);
    }

    /// A dropped connection: its socket is closed once its send is done,
    /// and its entry free once its receive is disarmed too.
    fn release(&mut self, id: usize) {
        let e = &mut self.conns[id];
        if e.sending {
            return;
        }
        if let Some(fd) = e.close.take() {
            self.queue(Sqe {
                opcode: OP_CLOSE,
                fd,
                user_data: IGNORE,
                ..Sqe::default()
            });
        }
        let e = &mut self.conns[id];
        if !e.armed && e.dropped {
            e.dropped = false;
            e.waker = None;
            self.free.push(id);
        }
    }

    fn read(&mut self, id: usize, buf: &mut Vec<u8>, cx: &Context) -> Poll<io::Result<usize>> {
        let e = &mut self.conns[id];
        if e.failed != 0 {
            return Poll::Ready(Err(io::Error::from_raw_os_error(e.failed)));
        }
        if !e.inbox.is_empty() {
            let n = e.inbox.len();
            buf.extend_from_slice(&e.inbox);
            e.inbox.clear();
            if e.inbox.capacity() > INBOX_LIMIT {
                e.inbox.shrink_to(BUF_SIZE);
            }
            if std::mem::take(&mut e.paused) {
                self.arm(id);
            }
            return Poll::Ready(Ok(n));
        }
        match e.ended {
            Some(errno) => Poll::Ready(Entry::result(errno).map(|()| 0)),
            None => {
                e.wants_recv = true;
                e.wait(cx);
                Poll::Pending
            }
        }
    }

    fn send(&mut self, id: usize, buf: &mut Vec<u8>) {
        if buf.is_empty() {
            return;
        }
        let e = &mut self.conns[id];
        debug_assert!(!e.sending && e.out.is_empty());
        std::mem::swap(&mut e.out, buf);
        (e.sent, e.sending, e.since) = (0, true, http::seconds());
        self.sending += 1;
        self.push_send(id);
    }

    fn flushed(&mut self, id: usize, cx: &Context) -> Poll<io::Result<()>> {
        let e = &mut self.conns[id];
        if e.sending {
            e.wants_send = true;
            e.wait(cx);
            return Poll::Pending;
        }
        Poll::Ready(Entry::result(e.failed))
    }

    /// Stops receiving for good, and once the kernel is done, moves what
    /// came onto `early`.
    fn detach(&mut self, id: usize, early: &mut Vec<u8>, cx: &Context) -> Poll<io::Result<()>> {
        let e = &mut self.conns[id];
        e.detached = true;
        if e.armed {
            if !e.cancelling {
                self.cancel(id, false);
            }
            let e = &mut self.conns[id];
            e.wants_recv = true;
            e.wait(cx);
            return Poll::Pending;
        }
        early.append(&mut e.inbox);
        Poll::Ready(Entry::result(e.ended.unwrap_or(0)))
    }

    fn close(&mut self, id: usize, fd: Option<RawFd>) {
        let e = &mut self.conns[id];
        (e.dropped, e.close, e.waker) = (true, fd, None);
        if e.armed && !e.cancelling {
            self.cancel(id, false);
        }
        self.release(id);
    }
}

/// A connection on its worker's ring. Like the task that holds it, it stays
/// on the worker's thread.
pub(crate) struct Sock {
    id: usize,
    stream: Option<TcpStream>,
}

impl Sock {
    pub(crate) fn new(stream: TcpStream) -> Sock {
        let id = with(|w| w.open(stream.as_raw_fd()));
        Sock {
            id,
            stream: Some(stream),
        }
    }

    /// Appends what came to `buf`: `Ok(0)` once the peer has closed. Safe to
    /// drop unfinished: what comes meanwhile waits for the next call.
    pub(crate) async fn read(&mut self, buf: &mut Vec<u8>) -> io::Result<usize> {
        poll_fn(|cx| with(|w| w.read(self.id, buf, cx))).await
    }

    /// Queues all of `buf` to go with the turn's other sends, and leaves
    /// `buf` empty (it gets the buffer the last send used). A failure shows
    /// in the next call.
    pub(crate) async fn write(&mut self, buf: &mut Vec<u8>) -> io::Result<()> {
        self.flush().await?;
        with(|w| w.send(self.id, buf));
        Ok(())
    }

    /// Waits for the send under way.
    async fn flush(&self) -> io::Result<()> {
        poll_fn(|cx| with(|w| w.flushed(self.id, cx))).await
    }

    /// Ends the sending side, once all of it has been sent.
    pub(crate) async fn shutdown(&mut self) {
        if self.flush().await.is_ok()
            && let Some(s) = &self.stream
        {
            let _ = s.shutdown(Shutdown::Write);
        }
    }

    /// The socket as tokio's, once the ring is done with it, and `early`
    /// with what the ring received that was not read on its end.
    pub(crate) async fn into_tcp(
        mut self,
        mut early: Vec<u8>,
    ) -> io::Result<(tokio::net::TcpStream, Vec<u8>)> {
        self.flush().await?;
        poll_fn(|cx| with(|w| w.detach(self.id, &mut early, cx))).await?;
        let stream = self.stream.take().ok_or(io::ErrorKind::NotConnected)?;
        stream.set_nonblocking(true)?;
        Ok((tokio::net::TcpStream::from_std(stream)?, early))
    }
}

impl Drop for Sock {
    fn drop(&mut self) {
        let _ = WORKER.try_with(|w| {
            if let Ok(mut w) = w.try_borrow_mut()
                && let Some(w) = w.as_mut()
            {
                w.close(self.id, self.stream.take().map(IntoRawFd::into_raw_fd));
            }
        });
    }
}

/// Proves that a ring lending its buffers this way works here, through the
/// code a worker runs: a throwaway ring, enabled on this thread, receives
/// on a multishot receive into a lent buffer, gives it back and sends a
/// reply; twice, so the second receive may take a buffer given back. Well
/// under a millisecond; leaves no descriptor open.
fn check() -> io::Result<()> {
    use std::io::{Read, Write};
    let (ours, mut peer) = UnixStream::pair().map_err(|e| step("a socket pair", e))?;
    peer.set_read_timeout(Some(Duration::from_secs(1)))?;
    let ring = Ring::new()?;
    register::<()>(&ring.fd, REGISTER_ENABLE_RINGS, None, 0)
        .map_err(|e| step("enabling the ring", e))?;
    let mut w = Worker::new(ring, -1);
    let id = w.open(ours.as_raw_fd());
    let deadline = std::time::Instant::now() + Duration::from_secs(1);
    // One turn, then what went wrong with the connection, if anything.
    let turn = |w: &mut Worker, what: &str| {
        w.turn().map_err(|e| step("entering the ring", e))?;
        let e = &w.conns[id];
        let errno = [e.ended.unwrap_or(0), e.failed];
        match errno.into_iter().find(|&e| e != 0) {
            Some(e) => Err(step(what, io::Error::from_raw_os_error(e))),
            None if e.ended.is_some() && e.inbox.is_empty() => {
                Err(step(what, io::ErrorKind::UnexpectedEof.into()))
            }
            None if std::time::Instant::now() > deadline => {
                Err(step(what, io::ErrorKind::TimedOut.into()))
            }
            None => Ok(()),
        }
    };
    for round in 1..=2u8 {
        peer.write_all(&[round; 3])?;
        let mut got = Vec::new();
        while got.len() < 3 {
            turn(&mut w, "receiving")?;
            got.append(&mut w.conns[id].inbox);
        }
        w.send(id, &mut got);
        while w.conns[id].sending {
            turn(&mut w, "sending")?;
        }
        let mut back = [0; 3];
        peer.read_exact(&mut back)
            .map_err(|e| step("reading the reply", e))?;
        if back != [round; 3] {
            return Err(step("reading the reply", io::ErrorKind::InvalidData.into()));
        }
    }
    Ok(())
}

/// `n` workers' rings, once `check` proves one works here; or why io_uring
/// with a buffer ring does not.
fn choose(n: usize) -> io::Result<Vec<Ring>> {
    check()?;
    (0..n).map(|_| Ring::new()).collect()
}

/// The workers' rings, or `None` for an epoll each (`epoll.rs`): asked for
/// with `WISP_IO=epoll`, or io_uring does not work here (`choose`). One line
/// on stderr says which. With `WISP_IO=uring`, io_uring not working is a
/// failure, with the reason.
pub(crate) fn rings(n: usize) -> Option<Vec<Ring>> {
    let asked =
        crate::setting::<String>("WISP_IO", "epoll or uring").map(|v| v.to_ascii_lowercase());
    match asked.as_deref() {
        Some("epoll") => {
            http::log(format_args!("wisp: io: epoll (asked by WISP_IO)"));
            return None;
        }
        None | Some("uring") => {}
        Some(v) => crate::fail(&format!("WISP_IO is {v:?}, which is not epoll or uring")),
    }
    match choose(n) {
        Ok(rings) => {
            http::log(format_args!("wisp: io: io_uring"));
            Some(rings)
        }
        Err(e) if asked.is_some() => crate::fail(&format!(
            "WISP_IO is uring, but io_uring does not work here: {e}\n  It needs Linux 6.1 or later with buffer rings, not refused by seccomp or the io_uring_disabled sysctl. Unset WISP_IO to use epoll."
        )),
        Err(e) => {
            http::log(format_args!("wisp: io: epoll (io_uring: {e})"));
            None
        }
    }
}

/// A listener on `addr` that shares its port with the other workers'. The
/// sockets it accepts inherit its `TCP_NODELAY`.
pub(crate) fn listen(addr: SocketAddr) -> io::Result<TcpListener> {
    let family = if addr.is_ipv4() {
        libc::AF_INET
    } else {
        libc::AF_INET6
    };
    // SAFETY: socket(2), no pointers.
    let fd = owned(unsafe { libc::socket(family, libc::SOCK_STREAM | libc::SOCK_CLOEXEC, 0) })?;
    let s = fd.as_raw_fd();
    let on: libc::c_int = 1;
    for (level, name) in [
        (libc::SOL_SOCKET, libc::SO_REUSEADDR),
        (libc::SOL_SOCKET, libc::SO_REUSEPORT),
        (libc::IPPROTO_TCP, libc::TCP_NODELAY),
    ] {
        let len = size_of_val(&on) as libc::socklen_t;
        // SAFETY: an int option, `on` alive for the call.
        if unsafe { libc::setsockopt(s, level, name, (&raw const on).cast(), len) } < 0 {
            return Err(io::Error::last_os_error());
        }
    }
    let bound = match addr {
        SocketAddr::V4(a) => {
            let sin = libc::sockaddr_in {
                sin_family: libc::AF_INET as libc::sa_family_t,
                sin_port: a.port().to_be(),
                sin_addr: libc::in_addr {
                    s_addr: u32::from_ne_bytes(a.ip().octets()),
                },
                sin_zero: [0; 8],
            };
            let len = size_of_val(&sin) as libc::socklen_t;
            // SAFETY: a sockaddr_in of `len` bytes, alive for the call.
            unsafe { libc::bind(s, (&raw const sin).cast(), len) }
        }
        SocketAddr::V6(a) => {
            let sin6 = libc::sockaddr_in6 {
                sin6_family: libc::AF_INET6 as libc::sa_family_t,
                sin6_port: a.port().to_be(),
                sin6_flowinfo: a.flowinfo(),
                sin6_addr: libc::in6_addr {
                    s6_addr: a.ip().octets(),
                },
                sin6_scope_id: a.scope_id(),
            };
            let len = size_of_val(&sin6) as libc::socklen_t;
            // SAFETY: a sockaddr_in6 of `len` bytes, alive for the call.
            unsafe { libc::bind(s, (&raw const sin6).cast(), len) }
        }
    };
    // SAFETY: listen(2), no pointers.
    if bound < 0 || unsafe { libc::listen(s, libc::SOMAXCONN) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(TcpListener::from(fd))
}

/// Lets the tasks woken meanwhile run, then goes on: a driver's yield
/// between two turns.
pub(crate) async fn yield_once() {
    let mut yielded = false;
    poll_fn(|cx| {
        if std::mem::replace(&mut yielded, true) {
            return Poll::Ready(());
        }
        cx.waker().wake_by_ref();
        Poll::Pending
    })
    .await;
}

/// A worker's driver: accepts on `listener`, gives each connection to
/// `accepted` (which starts its task), and turns the ring whenever a
/// connection queued something or the kernel has completions. Stops
/// accepting once the server is stopping; runs until the process ends.
pub(crate) async fn serve(mut ring: Ring, listener: TcpListener, accepted: fn(TcpStream)) {
    if let Err(e) = register::<()>(&ring.fd, REGISTER_ENABLE_RINGS, None, 0) {
        crate::fail(&format!("io_uring stopped working: {e}"));
    }
    let eventfd = ring
        .eventfd
        .take()
        .map(|fd| AsyncFd::with_interest(fd, Interest::READABLE));
    let Some(Ok(eventfd)) = eventfd else {
        crate::fail("io_uring stopped working: its eventfd cannot be watched");
    };
    WORKER.set(Some(Worker::new(ring, listener.as_raw_fd())));
    with(Worker::accept);
    let mut listener = Some(listener);
    let mut stop = std::pin::pin!(http::stopped());
    let mut timer = std::pin::pin!(tokio::time::sleep(Duration::ZERO));
    // Accepting again after a failure (out of descriptors) waits until then.
    let mut retry: Option<Instant> = None;
    let mut fds = Vec::new();
    loop {
        if retry.is_some_and(|t| t <= Instant::now()) {
            retry = None;
            with(Worker::accept);
        }
        let more = with(|w| {
            w.woken = true;
            if let Err(e) = w.turn() {
                // `check` proved this ring's calls work at start.
                crate::fail(&format!("io_uring stopped working: {e}"));
            }
            std::mem::swap(&mut fds, &mut w.accepted);
            if !w.accepting && listener.is_some() && retry.is_none() {
                if w.accept_error != 0
                    && http::accept_failed(&io::Error::from_raw_os_error(w.accept_error))
                {
                    retry = Some(Instant::now() + Duration::from_millis(50));
                } else {
                    w.accept();
                }
            }
            w.ring.queued() > 0 || w.ring.behind()
        });
        for fd in fds.drain(..) {
            // SAFETY: a socket the kernel just accepted for us; nothing else owns it.
            accepted(unsafe { TcpStream::from_raw_fd(fd) });
        }
        if more {
            // Handling completions queued more (a send's rest, a rearm), or
            // the kernel left work for another call: the connections just
            // woken go first, then another turn. One turn a yield, so a busy
            // ring never starves the worker's other tasks.
            yield_once().await;
            continue;
        }
        poll_fn(|cx| {
            if listener.is_some() && stop.as_mut().poll(cx).is_ready() {
                // Refuses new connections at once; the accept ends with an
                // error, which is not a failure now.
                if let Some(l) = listener.take() {
                    // SAFETY: shutdown(2) on our listener.
                    unsafe { libc::shutdown(l.as_raw_fd(), libc::SHUT_RDWR) };
                }
                retry = None;
                return Poll::Ready(());
            }
            let (queued, sending) = with(|w| {
                if !w.driver.as_ref().is_some_and(|d| d.will_wake(cx.waker())) {
                    w.driver = Some(cx.waker().clone());
                }
                w.woken = false;
                (w.ring.queued() > 0, w.sending > 0)
            });
            if queued {
                return Poll::Ready(());
            }
            if let Poll::Ready(ready) = eventfd.poll_read_ready(cx) {
                // Cleared before the turn, so what comes during it wakes us again.
                if let Ok(mut guard) = ready {
                    guard.clear_ready();
                }
                return Poll::Ready(());
            }
            // Stalled sends are checked once a second, and accepting resumes
            // after a failure, without anything else to wake the driver.
            let due = retry.or_else(|| sending.then(|| Instant::now() + Duration::from_secs(1)));
            if let Some(due) = due {
                if timer.deadline() != due {
                    timer.as_mut().reset(due);
                }
                if timer.as_mut().poll(cx).is_ready() {
                    return Poll::Ready(());
                }
            }
            Poll::Pending
        })
        .await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};

    /// A worker serving each connection with `each`, or `None` where this
    /// kernel has no io_uring for it.
    fn server(each: fn(TcpStream)) -> Option<SocketAddr> {
        let ring = Ring::new().ok()?;
        let listener = listen("127.0.0.1:0".parse().unwrap()).unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            rt.block_on(async move {
                tokio::spawn(serve(ring, listener, each));
                std::future::pending::<()>().await
            });
        });
        Some(addr)
    }

    fn echo(s: TcpStream) {
        let mut sock = Sock::new(s);
        tokio::spawn(async move {
            let mut buf = Vec::new();
            while let Ok(n) = sock.read(&mut buf).await
                && n > 0
                && sock.write(&mut buf).await.is_ok()
            {}
            sock.shutdown().await;
        });
    }

    fn pattern(n: usize) -> Vec<u8> {
        (0..n).map(|i| (i % 251) as u8).collect()
    }

    fn connect(addr: SocketAddr) -> std::net::TcpStream {
        let c = std::net::TcpStream::connect(addr).unwrap();
        c.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
        c
    }

    #[test]
    fn the_check_passes_where_the_kernel_takes_a_ring() {
        if Ring::new().is_ok() {
            check().unwrap();
        }
    }

    #[test]
    fn choose_gives_every_worker_a_ring_or_says_why() {
        match choose(3) {
            Ok(rings) => assert_eq!(rings.len(), 3),
            Err(e) => assert!(Ring::new().is_err() || check().is_err(), "{e}"),
        }
    }

    #[test]
    fn listeners_share_a_port() {
        let a = listen("127.0.0.1:0".parse().unwrap()).unwrap();
        let b = listen(a.local_addr().unwrap()).unwrap();
        assert_eq!(a.local_addr().unwrap(), b.local_addr().unwrap());
    }

    #[test]
    fn echoes_megabytes_in_order() {
        // More than the socket buffers and the inbox limit hold at once:
        // partial sends, paused receives, buffers running out.
        let Some(addr) = server(echo) else {
            return;
        };
        let sent = pattern(8 << 20);
        let mut c = connect(addr);
        let mut w = c.try_clone().unwrap();
        let data = sent.clone();
        let writer = std::thread::spawn(move || {
            w.write_all(&data).unwrap();
            w.shutdown(Shutdown::Write).unwrap();
        });
        let mut got = Vec::new();
        c.read_to_end(&mut got).unwrap();
        writer.join().unwrap();
        assert!(got == sent, "{} bytes back of {}", got.len(), sent.len());
    }

    #[test]
    fn many_connections_at_once() {
        let Some(addr) = server(echo) else {
            return;
        };
        let mut conns: Vec<_> = (0..300).map(|_| connect(addr)).collect();
        for round in 0..3u8 {
            for (i, c) in conns.iter_mut().enumerate() {
                c.write_all(format!("{round}:{i};").as_bytes()).unwrap();
            }
            for (i, c) in conns.iter_mut().enumerate() {
                let want = format!("{round}:{i};");
                let mut got = vec![0; want.len()];
                c.read_exact(&mut got).unwrap();
                assert_eq!(got, want.as_bytes());
            }
        }
    }

    #[test]
    fn a_response_outlives_its_socket() {
        // Queued, then the socket dropped at once: all of it still arrives,
        // then the end.
        fn answer(s: TcpStream) {
            let mut sock = Sock::new(s);
            tokio::spawn(async move {
                let mut buf = pattern(4 << 20);
                let _ = sock.write(&mut buf).await;
            });
        }
        let Some(addr) = server(answer) else {
            return;
        };
        let mut c = connect(addr);
        std::thread::sleep(Duration::from_millis(200));
        let mut got = Vec::new();
        c.read_to_end(&mut got).unwrap();
        assert!(got == pattern(4 << 20), "{} bytes", got.len());
    }

    #[test]
    fn a_socket_goes_to_tokio_with_what_came() {
        fn handover(s: TcpStream) {
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            let mut sock = Sock::new(s);
            tokio::spawn(async move {
                let mut buf = Vec::new();
                while buf.len() < 5 {
                    sock.read(&mut buf).await.unwrap();
                }
                sock.write(&mut b"ok".to_vec()).await.unwrap();
                // What comes now lands in the ring's inbox.
                tokio::time::sleep(Duration::from_millis(100)).await;
                let (mut tcp, mut early) = sock.into_tcp(Vec::new()).await.unwrap();
                while early.len() < 6 {
                    tcp.read_buf(&mut early).await.unwrap();
                }
                tcp.write_all(&early).await.unwrap();
            });
        }
        let Some(addr) = server(handover) else {
            return;
        };
        let mut c = connect(addr);
        c.write_all(b"hello").unwrap();
        let mut ok = [0; 2];
        c.read_exact(&mut ok).unwrap();
        assert_eq!(&ok, b"ok");
        c.write_all(b"world!").unwrap();
        let mut back = [0; 6];
        c.read_exact(&mut back).unwrap();
        assert_eq!(&back, b"world!");
    }
}
