use crate::sync::Arc;
use crate::cell::UnsafeCell;
use crate::futex::atomic::{Packable, Atomic};
use crate::futex::{Futex, WaitImpl, EnterResult};
use crate::futex::state::CopyWaker;
use crate::futex::atomic_impl::{usize2, IsAtomic};
use crate::sync::atomic::AtomicBool;
use std::marker::PhantomData;
use std::sync::atomic::Ordering::AcqRel;
use crate::future::poll_fn;
use crate::sync::atomic::Ordering::Release;
use std::mem::MaybeUninit;
use std::task::Poll;
use crate::sync::atomic::Ordering::Acquire;
use std::iter::repeat_with;
use crate::sync::atomic::Ordering::Relaxed;

pub struct Sender<T>(Arc<Inner<T>>);

pub struct Receiver<T>(Arc<Inner<T>>);

struct Bucket<T> {
    filled: AtomicBool,
    data: UnsafeCell<MaybeUninit<T>>,
}

#[derive(Clone, Copy, Debug)]
struct SendState {
    index: usize,
    size: usize,
}

#[derive(Clone, Copy, Debug)]
enum RecvState {
    Waiting(CopyWaker),
    Dirty,
}

struct Inner<T> {
    buckets: Vec<Bucket<T>>,
    send_queue: Futex,
    recv_state: Atomic<RecvState>,
    recv_head: UnsafeCell<usize>,
}

pub fn channel<T>(cap: usize) -> (Sender<T>, Receiver<T>) {
    let inner = Arc::new(Inner::new(cap));
    (Sender(inner.clone()), Receiver(inner))
}

impl<T> Inner<T> {
    pub fn new(cap: usize) -> Self {
        let buckets =
            repeat_with(|| Bucket {
                filled: AtomicBool::new(false),
                data: UnsafeCell::new(MaybeUninit::uninit()),
            })
                .take(cap)
                .collect();
        Inner {
            buckets,
            send_queue: Futex::new(SendState { index: 0, size: 0 }),
            recv_state: Atomic::new(RecvState::Dirty),
            recv_head: UnsafeCell::new(0),
        }
    }
    pub async unsafe fn send(&self, msg: T) {
        let mut bucket_index = None;
        self.send_queue.wait(WaitImpl {
            enter: |state: SendState| {
                if state.size == self.buckets.len() {
                    bucket_index = None;
                    EnterResult { userdata: None, pending: true }
                } else {
                    bucket_index = Some(state.index);
                    EnterResult { userdata: Some(SendState { index: state.index + 1, size: state.size + 1 }), pending: false }
                }
            },
            post_enter: Some(|| {}),
            exit: || {},
            phantom: PhantomData,
        }).await;
        if let Some(bucket_index) = bucket_index {
            self.buckets[bucket_index].data.with_mut(|data|
                (*data).write(msg)
            );
            self.buckets[bucket_index].filled.store(true, Release);
            match self.recv_state.swap(RecvState::Dirty, AcqRel) {
                RecvState::Waiting(waker) => waker.into_waker().wake(),
                RecvState::Dirty => {}
            }
        }
    }
    pub async unsafe fn recv(&self) -> T {
        poll_fn(|cx| {
            let head = self.recv_head.with_mut(|x| *x);
            let bucket = &self.buckets[head] as *const Bucket<T>;
            if !(*bucket).filled.load(Acquire) {
                self.recv_state.store(RecvState::Waiting(CopyWaker::from_waker(cx.waker().clone())), Release);
                if !(*bucket).filled.load(Acquire) {
                    return Poll::Pending;
                }
            }
            let bucket = bucket as *mut Bucket<T>;
            (*bucket).filled.store_mut(false);
            let result = (*bucket).data.with_mut(|x| (*x).assume_init_read());
            self.recv_head.with_mut(|x| *x = head + 1);
            let lock = self.send_queue.lock_state();
            let waiter = lock.with_mut(|state| {
                if let Some(waiter) = self.send_queue.pop(&mut *state) {
                    return Some(waiter);
                }
                self.send_queue.update_flip(&mut *state, |atom: SendState, queued| {
                    if !queued {
                        Some(SendState { index: atom.index, size: atom.size - 1 })
                    } else {
                        None
                    }
                });
                self.send_queue.pop(&mut *state)
            });
            if let Some(waiter) = waiter {
                (*bucket).filled.store_mut(true);
                (*bucket).data.with_mut(|x| (*x).write(todo!()));
                waiter.done();
            }
            Poll::Ready(result)
        }).await
    }
}

impl Packable for SendState {
    type Raw = usize;
    unsafe fn encode(val: Self) -> Self::Raw { todo!() }
    unsafe fn decode(val: Self::Raw) -> Self { todo!() }
}

impl Packable for RecvState {
    type Raw = usize2;
    unsafe fn encode(val: Self) -> Self::Raw { todo!() }
    unsafe fn decode(val: Self::Raw) -> Self { todo!() }
}