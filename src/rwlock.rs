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

use crate::futex::{Atomic, Packable};
use crate::futex::{AtomicUsize2, usize2, usize_half};
use crate::cell::UnsafeCell;
use crate::futex::{Futex, WaitFuture, WaitAction, Flow};
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
    pub(crate) futex: Futex,
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
        unsafe {
            test_println!("Read locking");
            self.futex.wait(
                0,
                READ_QUEUE,
                |atom: Atom| {
                    if atom.writers > 0 {
                        WaitAction { update: None, flow: Flow::Pending(()) }
                    } else {
                        WaitAction {
                            update: Some(Atom { readers: atom.readers + 1, ..atom }),
                            flow: Flow::Ready(()),
                        }
                    }
                },
                |_| test_println!("Read waiting"),
                |_| self.read_unlock(),
                |_| (),
            ).await;
            test_println!("Read locked");
            ReadGuard { mutex: self }
        }
    }

    pub async fn write<'a>(&'a self) -> WriteGuard<'a, T> {
        unsafe {
            test_println!("Write locking");
            self.futex.wait(
                0,
                WRITE_QUEUE,
                |atom: Atom| {
                    WaitAction {
                        update: Some(Atom { writers: atom.writers + 1, ..atom }),
                        flow: if atom == (Atom { readers: 0, writers: 0 }) {
                            Flow::Ready(())
                        } else {
                            Flow::Pending(())
                        },
                    }
                },
                |_| test_println!("Write waiting"),
                |_| self.write_unlock(),
                |_| self.write_abort(),
            ).await;
            test_println!("Write locked");
            WriteGuard { mutex: self }
        }
    }

    pub(crate) unsafe fn write_abort(&self) {
        todo!()
    }

    pub(crate) unsafe fn read_unlock(&self) {
        if self.futex.fetch_update(|mut atom: Atom| {
            if atom.readers == 1 && atom.writers > 0 {
                None
            } else {
                Some(Atom { readers: atom.readers - 1, ..atom })
            }
        }).is_ok() {
            return;
        }
        let lock = self.futex.lock();
        lock.fetch_update_enqueue(|atom: Atom, queued| {
            Some(Atom { readers: 0, ..atom })
        });
        if let Some(writer) = lock.pop(WRITE_QUEUE) {
            writer.wake();
        } else {
            assert!(lock.pop(READ_QUEUE).is_none());
        }
    }

    pub(crate) unsafe fn write_unlock(&self) {
        let lock = self.futex.lock();
        let mut waking = false;
        lock.fetch_update_enqueue(|atom: Atom, queued| {
            assert_eq!(atom.readers, 0);
            if atom.writers == 1 {
                if queued {
                    // Downgrade to a read lock so it's safe to wake other readers.
                    waking = true;
                    Some(Atom { writers: 0, readers: 1 })
                } else {
                    waking = false;
                    Some(Atom { writers: 0, readers: 0 })
                }
            } else {
                waking = true;
                Some(Atom { writers: atom.writers - 1, readers: 0 })
            }
        });
        if waking {
            if let Some(writer) = lock.pop(WRITE_QUEUE) {
                writer.wake();
                return;
            }
            let readers = lock.pop_many(usize::MAX, READ_QUEUE);
            let count = readers.count();
            self.futex.fetch_update(|atom: Atom| {
                Some(Atom { readers: atom.readers + count - 1, ..atom })
            }).ok();
            for reader in readers {
                reader.wake();
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