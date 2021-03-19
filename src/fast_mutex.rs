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

use crate::cell::UnsafeCell;
use crate::future::Future;
use crate::sync::Arc;
use crate::sync::atomic::AtomicBool;
use crate::sync::atomic::AtomicUsize;
use crate::sync::atomic::Ordering::{AcqRel, Acquire, Relaxed, Release};
use crate::util::{AsyncFnOnce, Bind, FnOnceExt};
use crate::futex::{Futex, WaitAction, Packable, Flow, usize_half, Atomic};

#[derive(Debug)]
pub struct Mutex<T> {
    futex: Futex,
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
        unsafe {
            if let Flow::Ready(_) = self.futex.wait(
                0,
                0,
                |atom: Atom| {
                    if !atom.locked {
                        WaitAction {
                            update: Some(Atom { locked: true, ..atom }),
                            flow: Flow::Ready(()),
                        }
                    } else {
                        WaitAction {
                            update: Some(Atom {
                                waiters: atom.waiters.checked_add(1).unwrap(),
                                ..atom
                            }),
                            flow: Flow::Pending(()),
                        }
                    }
                },
                |_| (),
                |_| todo!(),
                |_| todo!(),
            ).await {
                return MutexGuard { mutex: self };
            }
            loop {
                if let Flow::Ready(_) = self.futex.wait(
                    0,
                    0,
                    |atom: Atom| {
                        if !atom.locked {
                            WaitAction {
                                update: Some(Atom {
                                    locked: true,
                                    waiters: atom.waiters - 1,
                                    waking: false,
                                }),
                                flow: Flow::Ready(()),
                            }
                        } else {
                            WaitAction {
                                update: Some(Atom {
                                    waking: false,
                                    ..atom
                                }),
                                flow: Flow::Pending(()),
                            }
                        }
                    },
                    |_| (),
                    |_| todo!(),
                    |_| todo!(),
                ).await {
                    return MutexGuard { mutex: self };
                }
            }
        }
    }

    pub fn unlock<'a>(&'a self) {
        unsafe {
            let mut futex_atom = self.futex.load(Relaxed);
            loop {
                let atom = futex_atom.inner::<Atom>();
                if atom.waking || atom.waiters == 0 {
                    if self.futex.cmpxchg_weak(
                        &mut futex_atom,
                        Atom { locked: false, ..atom },
                        Release, Relaxed) {
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
                atom = futex_atom.inner::<Atom>();
                if had_waiters {
                    if self.futex.cmpxchg_weak(
                        &mut futex_atom,
                        Atom { locked: false, waking: true, ..atom },
                        Release, Relaxed) {

                        break;
                    }
                } else if futex_atom.has_new_waiters() {
                    if queue.cmpxchg_enqueue_weak(
                        &mut futex_atom,
                        Atom { locked: false, waking: true, ..atom },
                        AcqRel, Relaxed,
                    ) {
                        break;
                    }
                } else {
                    assert_eq!(atom.waiters, 0);
                    if self.futex.cmpxchg_weak(
                        &mut futex_atom,
                        Atom { locked: false, waking: false, waiters: 0 },
                        Release, Relaxed,
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