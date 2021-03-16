use crate::sync::Arc;
use crate::cell::UnsafeCell;
use crate::futex::atomic::{Packable, Atomic};
use crate::futex::{Futex, WaitAction, Flow};
use crate::futex::state::{CopyWaker, PANIC_WAKER_VTABLE};
use crate::futex::atomic_impl::{usize2, IsAtomic, usize_half};
use crate::sync::atomic::AtomicBool;
use std::marker::PhantomData;
use crate::sync::atomic::Ordering::AcqRel;
use crate::future::poll_fn;
use crate::sync::atomic::Ordering::Release;
use std::mem::MaybeUninit;
use std::task::{Poll, RawWakerVTable};
use crate::sync::atomic::Ordering::Acquire;
use std::iter::repeat_with;
use crate::sync::atomic::Ordering::Relaxed;
use std::mem;
use std::ptr::null;
use crate::test_println;
use std::sync::mpsc::{SendError, RecvError};

#[derive(Clone, Debug)]
pub struct Sender<T>(Arc<Inner<T>>);

#[derive(Debug)]
pub struct Receiver<T>(Arc<Inner<T>>);

#[derive(Debug)]
struct Bucket<T> {
    filled: AtomicBool,
    data: UnsafeCell<MaybeUninit<T>>,
}

#[derive(Clone, Copy, Debug)]
struct SendState {
    write_head: usize,
    size: usize,
}

#[derive(Clone, Copy, Debug)]
enum RecvState {
    Waiting(CopyWaker),
    Dirty,
}

#[derive(Debug)]
struct Inner<T> {
    buckets: Vec<Bucket<T>>,
    send_queue: Futex,
    recv_state: Atomic<RecvState>,
    recv_head: UnsafeCell<usize>,
}

struct Message<T> (UnsafeCell<Option<T>>);

pub fn channel<T>(cap: usize) -> (Sender<T>, Receiver<T>) {
    let inner = Arc::new(Inner::new(cap));
    (Sender(inner.clone()), Receiver(inner))
}

impl<T> Inner<T> {
    pub fn new(cap: usize) -> Self {
        assert!(cap > 0);
        assert!(cap <= usize_half::MAX as usize);
        let buckets =
            repeat_with(|| Bucket {
                filled: AtomicBool::new(false),
                data: UnsafeCell::new(MaybeUninit::uninit()),
            })
                .take(cap)
                .collect();
        Inner {
            buckets,
            send_queue: Futex::new(SendState { write_head: 0, size: 0 }, 1),
            recv_state: Atomic::new(RecvState::Dirty),
            recv_head: UnsafeCell::new(0),
        }
    }

    pub async unsafe fn send(&self, msg: T) -> Result<(), SendError<T>> {
        let cap = self.buckets.len();
        let mut msg: Message<T> = Message(UnsafeCell::new(Some(msg)));
        let msg_ptr = &msg as *const Message<T> as usize;
        let selected = self.send_queue.wait(
            msg_ptr,
            0,
            |mut state: SendState| {
                if state.size < cap {
                    let selected = state.write_head;
                    state.write_head = (state.write_head + 1) % cap;
                    state.size += 1;
                    WaitAction { update: Some(state), flow: Flow::Ready(selected) }
                } else {
                    WaitAction { update: None, flow: Flow::Pending(()) }
                }
            },
            |_| (),
            |_| assert!(msg.is_none()),
            |_| (),
        ).await;
        if let Flow::Ready(selected) = selected {
            let msg = msg.take();
            self.buckets[selected].data.with_mut(|data|
                (*data).write(msg)
            );
            self.buckets[selected].filled.store(true, Release);
            match self.recv_state.swap(RecvState::Dirty, AcqRel) {
                RecvState::Waiting(waker) => waker.into_waker().wake(),
                RecvState::Dirty => {}
            }
            test_println!("Filled {:?}", &self.buckets[selected] as *const Bucket<T>);
        }
        Ok(())
    }
    pub async unsafe fn recv(&self) -> T {
        poll_fn(|cx| {
            let head = self.recv_head.with_mut(|x| *x);
            let bucket = &self.buckets[head] as *const Bucket<T>;
            if !(*bucket).filled.load(Acquire) {
                match self.recv_state.swap(RecvState::Waiting(CopyWaker::from_waker(cx.waker().clone())), AcqRel) {
                    RecvState::Waiting(old) => mem::drop(old.into_waker()),
                    RecvState::Dirty => {}
                }
                if !(*bucket).filled.load(Acquire) {
                    test_println!("Receive blocked {:?}", bucket);
                    return Poll::Pending;
                }
                match self.recv_state.swap(RecvState::Dirty, AcqRel) {
                    RecvState::Waiting(old) => mem::drop(old.into_waker()),
                    RecvState::Dirty => {}
                }
            }
            let bucket = bucket as *mut Bucket<T>;
            (*bucket).filled.store_mut(false);
            test_println!("Unfilled {:?}", head);
            let result = (*bucket).data.with_mut(|x| (*x).assume_init_read());
            self.recv_head.with_mut(|x| *x = (head + 1) % self.buckets.len());
            let state = self.send_queue.lock_state();
            self.send_queue.update_flip(&*state, |atom: SendState, queued| {
                if queued {
                    assert_eq!(atom.size, self.buckets.len());
                    Some(SendState { write_head: (atom.write_head + 1) % self.buckets.len(), size: atom.size })
                } else {
                    Some(SendState { write_head: atom.write_head, size: atom.size - 1 })
                }
            });
            let waiter = self.send_queue.pop(&*state, 0);
            if let Some(waiter) = waiter {
                (*bucket).filled.store_mut(true);
                test_println!("Backfilled {:?}",head);
                (*bucket).data.with_mut(|bucket| {
                    let message = waiter.message() as *const Message<T>;
                    (*bucket).write((*message).take())
                });
                waiter.done();
            }
            Poll::Ready(result)
        }).await
    }
}

impl<T> Receiver<T> {
    pub async fn recv(&mut self) -> Result<T, RecvError> {
        unsafe { Ok(self.0.recv().await) }
    }
}

impl<T> Sender<T> {
    pub async fn send(&self, msg: T) -> Result<(), SendError<T>> {
        unsafe { self.0.send(msg).await }
    }
}

impl Packable for SendState {
    type Raw = usize;
    unsafe fn encode(val: Self) -> Self::Raw {
        mem::transmute((val.size as usize_half, val.write_head as usize_half))
    }
    unsafe fn decode(val: Self::Raw) -> Self {
        let (size, write_head): (usize_half, usize_half) = mem::transmute(val);
        Self { size: size as usize, write_head: write_head as usize }
    }
}

static WAITER_NONE_TAG: RawWakerVTable = PANIC_WAKER_VTABLE;

impl Packable for RecvState {
    type Raw = usize2;
    unsafe fn encode(val: Self) -> Self::Raw {
        mem::transmute(match val {
            RecvState::Waiting(waker) => waker,
            RecvState::Dirty => CopyWaker(null(), &WAITER_NONE_TAG)
        })
    }
    unsafe fn decode(val: Self::Raw) -> Self {
        let waker: CopyWaker = mem::transmute(val);
        if waker.1 == &WAITER_NONE_TAG as *const RawWakerVTable {
            RecvState::Dirty
        } else {
            RecvState::Waiting(waker)
        }
    }
}

impl<T> Drop for Inner<T> {
    fn drop(&mut self) {
        match self.recv_state.load_mut() {
            RecvState::Waiting(_) => panic!(),
            RecvState::Dirty => {}
        }
    }
}

impl<T> Message<T> {
    unsafe fn new(x: T) -> Self {
        Message(UnsafeCell::new(Some(x)))
    }
    unsafe fn is_none(&self) -> bool {
        self.0.with_mut(|x| (*x).is_none())
    }
    unsafe fn take(&self) -> T {
        self.0.with_mut(|x| (*x).take().unwrap())
    }
}

unsafe impl<T: Send> Send for Inner<T> {}

unsafe impl<T: Send> Sync for Inner<T> {}

unsafe impl<T: Send> Send for Message<T> {}

unsafe impl<T: Send> Sync for Message<T> {}