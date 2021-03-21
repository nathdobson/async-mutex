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

use crate::cell::UnsafeCell;
use crate::future::Future;
use crate::sync::Arc;
use crate::sync::atomic::AtomicBool;
use crate::sync::atomic::AtomicUsize;
use crate::sync::atomic::Ordering::{AcqRel, Acquire, Relaxed, Release};
use crate::util::{AsyncFnOnce, Bind, FnOnceExt};
use crate::futex::{Futex};
use crate::atomic::{Packable, AcquireT, RelaxedT, ReleaseT, AcqRelT};

#[derive(Debug)]
pub struct Mutex<T> {
    futex: Futex<Atom>,
    inner: UnsafeCell<T>,
}

#[derive(Debug)]
pub struct MutexGuard<'a, T> {
    pub(crate) mutex: &'a Mutex<T>,
}

#[derive(Copy, Clone, Eq, PartialEq, Debug)]
struct Atom {
    locked: bool,
    waking: bool,
    waiters: usize,
}

impl<T> Mutex<T> {
    pub fn new(inner: T) -> Self {
        Mutex {
            futex: Futex::new(Atom { locked: false, waking: false, waiters: 0 }, 1),
            inner: UnsafeCell::new(inner),
        }
    }

    pub fn get_mut(&mut self) -> &mut T {
        unsafe { self.inner.with_mut(|x| &mut *x) }
    }

    pub async fn lock<'a>(&'a self) -> MutexGuard<'a, T> {
        let mut futex_atom = self.futex.load(RelaxedT);
        let waiter = self.futex.waiter(0, 0);
        pin_mut!(waiter);
        loop {
            let atom = futex_atom.inner();
            if atom.locked {
                if self.futex.cmpxchg_wait_weak(
                    waiter.as_mut(),
                    &mut futex_atom,
                    Atom { waiters: atom.waiters.checked_add(1).unwrap(), ..atom },
                    ReleaseT,
                    RelaxedT,
                    || {},
                    || todo!(),
                    || todo!(),
                ).await {
                    break;
                }
            } else {
                if self.futex.cmpxchg_weak(
                    &mut futex_atom,
                    Atom { locked: true, ..atom },
                    AcquireT, RelaxedT,
                ) {
                    return MutexGuard { mutex: self };
                }
            }
        }
        loop {
            let waiter = self.futex.waiter(0, 0);
            pin_mut!(waiter);
            loop {
                let atom = futex_atom.inner();
                if atom.locked {
                    if self.futex.cmpxchg_wait_weak(
                        waiter.as_mut(),
                        &mut futex_atom,
                        Atom { waking: false, ..atom },
                        ReleaseT,
                        RelaxedT,
                        || {},
                        || todo!(),
                        || todo!(),
                    ).await {
                        break;
                    }
                } else {
                    if self.futex.cmpxchg_weak(
                        &mut futex_atom,
                        Atom { locked: true, waiters: atom.waiters - 1, waking: false },
                        AcquireT, RelaxedT,
                    ) {
                        return MutexGuard { mutex: self };
                    }
                }
            }
        }
    }

    pub fn unlock<'a>(&'a self) {
        let mut futex_atom = self.futex.load(RelaxedT);
        loop {
            let atom = futex_atom.inner();
            if atom.waking || atom.waiters == 0 {
                if self.futex.cmpxchg_weak(
                    &mut futex_atom,
                    Atom { locked: false, ..atom },
                    ReleaseT, RelaxedT) {
                    return;
                }
            } else if atom.waking {
                return;
            } else {
                break;
            }
        }
        let mut waiters = self.futex.lock();
        let mut queue = waiters.lock();
        let had_waiters = !queue.is_empty(0);
        let mut atom;
        loop {
            atom = futex_atom.inner();
            if had_waiters {
                if self.futex.cmpxchg_weak(
                    &mut futex_atom,
                    Atom { locked: false, waking: true, ..atom },
                    ReleaseT, RelaxedT) {
                    break;
                }
            } else if futex_atom.has_new_waiters() {
                if queue.cmpxchg_enqueue_weak(
                    &mut futex_atom,
                    Atom { locked: false, waking: true, ..atom },
                    AcqRelT, RelaxedT,
                ) {
                    break;
                }
            } else {
                assert_eq!(atom.waiters, 0);
                if self.futex.cmpxchg_weak(
                    &mut futex_atom,
                    Atom { locked: false, waking: false, waiters: 0 },
                    ReleaseT, RelaxedT,
                ) {
                    return;
                }
            }
        }
        if !atom.waking {
            let waiter = queue.pop(0).unwrap();
            mem::drop(queue);
            waiter.wake();
        }
    }
}

impl Packable for Atom {
    type Raw = usize;
    unsafe fn encode(val: Self) -> Self::Raw {
        val.locked as usize
            | (val.waking as usize) << 1
            | val.waiters << 2
    }
    unsafe fn decode(val: Self::Raw) -> Self {
        Self {
            locked: val & 1 == 1,
            waking: (val >> 1) & 1 == 1,
            waiters: val >> 2,
        }
    }
}

impl<'a, T> Drop for MutexGuard<'a, T> {
    fn drop(&mut self) {
        unsafe {
            self.mutex.unlock();
        }
    }
}

unsafe impl<T: Send> Send for Mutex<T> {}

unsafe impl<T: Send> Sync for Mutex<T> {}

unsafe impl<'a, T: Send> Send for MutexGuard<'a, T> {}

unsafe impl<'a, T: Send + Sync> Sync for MutexGuard<'a, T> {}

impl<'a, T> Deref for MutexGuard<'a, T> {
    type Target = T;
    fn deref(&self) -> &Self::Target { unsafe { &*self.mutex.inner.with_mut(|x| x) } }
}

impl<'a, T> DerefMut for MutexGuard<'a, T> {
    fn deref_mut(&mut self) -> &mut Self::Target { unsafe { &mut *self.mutex.inner.with_mut(|x| x) } }
}

impl<T: Default> Default for Mutex<T> {
    fn default() -> Self {
        Mutex::new(T::default())
    }
}

#[test]
fn test_atom() {
    use crate::futex::test_packable;
    test_packable(Atom { locked: false, waking: false, waiters: 0 });
    test_packable(Atom { locked: true, waking: false, waiters: 0 });
    test_packable(Atom { locked: false, waking: true, waiters: 0 });
    test_packable(Atom { locked: false, waking: true, waiters: usize::MAX >> 2 });
}