use std::backtrace::{Backtrace, BacktraceStatus};
use std::borrow::Borrow;
use std::collections::HashMap;
use std::lazy::SyncOnceCell;
use std::marker::PhantomData;
use std::mem;
use std::mem::{align_of, MaybeUninit, size_of};
use std::ops::{Deref, DerefMut};
use std::pin::Pin;
use std::process::abort;
use std::ptr::{null, null_mut};
use std::sync::Weak;
use std::task::{Context, Poll, RawWaker, RawWakerVTable};
use std::task::Waker;
use pin_utils::pin_mut;

use crate::atomic::{Atomic, Packable, AcquireT, RelaxedT, ReleaseT, AcqRelT};
use crate::atomic::{AtomicUsize2, usize2, usize_half};
use crate::cell::UnsafeCell;
use crate::futex::{Futex};
use crate::future::Future;
use crate::sync::Arc;
use crate::sync::atomic::AtomicBool;
use crate::sync::atomic::AtomicUsize;
use crate::sync::atomic::Ordering::{AcqRel, Acquire, Relaxed, Release};
//use crate::test_println;
use crate::util::{AsyncFnOnce, Bind, FnOnceExt};
use crate::futex::Waiter;

#[derive(Debug)]
pub struct RwLock<T> {
    pub(crate) futex: Futex<Atom>,
    inner: UnsafeCell<T>,
}

#[derive(Debug)]
pub struct ReadGuard<'a, T> {
    pub(crate) mutex: &'a RwLock<T>,
}

#[derive(Debug)]
pub struct WriteGuard<'a, T> {
    pub(crate) mutex: &'a RwLock<T>,
}

#[derive(Copy, Clone, Eq, PartialEq, Debug)]
pub struct Atom {
    // Number of active readers (excluding queued)
    readers: usize,
    // Number of writers (including queued)
    writers: usize,
}

const READ_QUEUE: usize = 0;
const WRITE_QUEUE: usize = 1;

impl<T> RwLock<T> {
    pub fn new(inner: T) -> Self {
        RwLock {
            futex: Futex::new(Atom { readers: 0, writers: 0 }, 2),
            inner: UnsafeCell::new(inner),
        }
    }

    pub fn get_mut(&mut self) -> &mut T {
        unsafe { self.inner.with_mut(|x| &mut *x) }
    }

    pub async fn read<'a>(&'a self) -> ReadGuard<'a, T> {
        test_println!("Read locking");
        let waiter = self.futex.waiter(0, READ_QUEUE);
        pin_mut!(waiter);
        let mut futex_atom = self.futex.load(RelaxedT);
        loop {
            let atom = futex_atom.inner();
            if atom.writers == 0 {
                if self.futex.cmpxchg_weak(
                    &mut futex_atom,
                    Atom { readers: atom.readers + 1, writers: 0 },
                    AcquireT, RelaxedT) {
                    break;
                }
            } else {
                if self.futex.cmpxchg_wait_weak(
                    waiter.as_mut(),
                    &mut futex_atom,
                    atom,
                    ReleaseT,
                    RelaxedT,
                    || {},
                    || unsafe { self.read_unlock() },
                    || {},
                ).await {
                    break;
                }
            }
        }
        test_println!("Read locked");
        ReadGuard { mutex: self }
    }

    pub async fn write<'a>(&'a self) -> WriteGuard<'a, T> {
        test_println!("Write locking");
        let waiter = self.futex.waiter(0, WRITE_QUEUE);
        pin_mut!(waiter);
        let mut futex_atom = self.futex.load(RelaxedT);
        loop {
            let atom = futex_atom.inner();
            if atom == (Atom { readers: 0, writers: 0 }) {
                if self.futex.cmpxchg_weak(
                    &mut futex_atom,
                    Atom { readers: 0, writers: 1 },
                    AcquireT, RelaxedT,
                ) {
                    break;
                }
            } else {
                if self.futex.cmpxchg_wait_weak(
                    waiter.as_mut(),
                    &mut futex_atom,
                    Atom { writers: atom.writers + 1, ..atom },
                    ReleaseT,
                    RelaxedT,
                    || test_println!("Write waiting"),
                    || unsafe { self.write_unlock() },
                    || unsafe { self.write_abort() },
                ).await {
                    break;
                }
            }
        }
        test_println!("Write locked");
        WriteGuard { mutex: self }
    }

    pub(crate) unsafe fn write_abort(&self) {
        todo!()
    }

    pub(crate) unsafe fn read_unlock(&self) {
        test_println!("Read unlocking");
        let mut futex_atom = self.futex.load(RelaxedT);
        loop {
            let atom = futex_atom.inner();
            if atom.readers == 1 && atom.writers > 0 {
                break;
            } else {
                if self.futex.cmpxchg_weak(
                    &mut futex_atom,
                    Atom { readers: atom.readers - 1, ..atom },
                    ReleaseT, RelaxedT) {
                    return;
                }
            }
        }
        let mut waiters = self.futex.lock();
        let mut queue = waiters.lock();
        let writers: usize;
        loop {
            let atom = futex_atom.inner();
            if queue.cmpxchg_enqueue_weak(&mut futex_atom, Atom { readers: 0, ..atom }, AcqRelT, RelaxedT) {
                writers = atom.writers;
                break;
            }
        }
        if let Some(writer) = queue.pop(WRITE_QUEUE) {
            assert!(writers > 0);
            mem::drop(queue);
            writer.wake();
        } else {
            assert_eq!(writers, 0);
            assert!(queue.pop(READ_QUEUE).is_none());
        }
    }

    pub(crate) unsafe fn write_unlock(&self) {
        test_println!("Write unlocking");
        let mut waiters = self.futex.lock();
        let mut queue = waiters.lock();
        let mut futex_atom = self.futex.load(RelaxedT);
        let has_readers = !queue.is_empty(READ_QUEUE);
        loop {
            let atom = futex_atom.inner();
            assert_eq!(atom.readers, 0);
            if atom.writers == 1 && !has_readers && !futex_atom.has_new_waiters() {
                mem::drop(queue);
                if self.futex.cmpxchg_weak(
                    &mut futex_atom,
                    Atom { writers: 0, readers: 0 },
                    ReleaseT, RelaxedT) {
                    test_println!("Write unlocked no waiters");
                    return;
                } else {
                    queue = waiters.lock();
                }
            } else if atom.writers == 1 {
                // Downgrade to a read lock so other readers don't think they're the leader.
                if queue.cmpxchg_enqueue_weak(
                    &mut futex_atom,
                    Atom { writers: 0, readers: 1 },
                    AcqRelT, RelaxedT) {
                    assert!(queue.pop(WRITE_QUEUE).is_none());
                    let mut readers = queue.pop_many(usize::MAX, READ_QUEUE);
                    mem::drop(queue);
                    let count: usize = (&readers).into_iter().count();
                    loop {
                        let atom = futex_atom.inner();
                        if self.futex.cmpxchg_weak(
                            &mut futex_atom,
                            Atom { readers: atom.readers + count - 1, ..atom },
                            RelaxedT, RelaxedT) {
                            break;
                        }
                    }
                    for reader in readers {
                        reader.wake();
                    }
                    test_println!("Write unlocked to readers");
                    return;
                }
            } else {
                if queue.cmpxchg_enqueue_weak(
                    &mut futex_atom,
                    Atom { writers: atom.writers - 1, ..atom },
                    AcqRelT, RelaxedT) {
                    assert!(atom.writers > 1);
                    let writer = queue.pop(WRITE_QUEUE).unwrap();
                    mem::drop(queue);
                    writer.wake();
                    test_println!("Write unlocked to writer");
                    return;
                }
            }
        }
    }
}

impl Packable for Atom {
    type Raw = usize;

    unsafe fn encode(val: Self) -> Self::Raw {
        mem::transmute((val.readers as usize_half, val.writers as usize_half))
    }

    unsafe fn decode(val: Self::Raw) -> Self {
        let (readers, writers): (usize_half, usize_half) = mem::transmute(val);
        Self { readers: readers as usize, writers: writers as usize }
    }
}

impl<'a, T> Drop for ReadGuard<'a, T> {
    fn drop(&mut self) {
        unsafe { self.mutex.read_unlock(); }
    }
}

impl<'a, T> Drop for WriteGuard<'a, T> {
    fn drop(&mut self) {
        unsafe { self.mutex.write_unlock(); }
    }
}

unsafe impl<T: Send> Send for RwLock<T> {}

unsafe impl<T: Send> Sync for RwLock<T> {}

unsafe impl<'a, T: Send + Sync> Send for ReadGuard<'a, T> {}

unsafe impl<'a, T: Send + Sync> Sync for ReadGuard<'a, T> {}

unsafe impl<'a, T: Send> Send for WriteGuard<'a, T> {}

unsafe impl<'a, T: Send + Sync> Sync for WriteGuard<'a, T> {}

impl<'a, T> Deref for ReadGuard<'a, T> {
    type Target = T;
    fn deref(&self) -> &Self::Target { unsafe { &*self.mutex.inner.with(|x| x) } }
}

impl<'a, T> Deref for WriteGuard<'a, T> {
    type Target = T;
    fn deref(&self) -> &Self::Target { unsafe { &*self.mutex.inner.with_mut(|x| x) } }
}

impl<'a, T> DerefMut for WriteGuard<'a, T> {
    fn deref_mut(&mut self) -> &mut Self::Target { unsafe { &mut *self.mutex.inner.with_mut(|x| x) } }
}

impl<T: Default> Default for RwLock<T> {
    fn default() -> Self {
        RwLock::new(T::default())
    }
}