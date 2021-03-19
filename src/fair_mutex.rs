use std::backtrace::{Backtrace, BacktraceStatus};
use std::borrow::Borrow;
use std::collections::HashMap;
use std::lazy::SyncOnceCell;
use std::marker::PhantomData;
use std::{mem, thread};
use std::mem::{align_of, MaybeUninit, size_of};
use std::ops::{Deref, DerefMut};
use std::pin::Pin;
use std::process::abort;
use std::ptr::{null, null_mut};
use std::sync::Weak;
use std::task::{Context, Poll, RawWaker, RawWakerVTable};
use std::task::Waker;

use crate::futex::{Atomic, Packable, FutexWaitersGuard, FutexQueueGuard};
use crate::futex::{AtomicUsize2, usize2};
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
use core::time::Duration;
use crate::sync::atomic::Ordering::SeqCst;
use crate::futex::FutexAtom;

#[derive(Debug)]
pub struct Mutex<T> {
    pub(crate) futex: Futex,
    inner: UnsafeCell<T>,
}

#[derive(Debug)]
pub struct MutexGuard<'a, T> {
    pub(crate) mutex: &'a Mutex<T>,
}

impl<T> Mutex<T> {
    pub fn new(inner: T) -> Self {
        Mutex { futex: Futex::new(0usize, 1), inner: UnsafeCell::new(inner) }
    }

    pub fn get_mut(&mut self) -> &mut T {
        unsafe { self.inner.with_mut(|x| &mut *x) }
    }

    pub async fn lock<'a>(&'a self) -> MutexGuard<'a, T> {
        unsafe {
            self.futex.wait(
                0,
                0,
                |locked: usize| {
                    if locked == 0 {
                        WaitAction { update: Some(1), flow: Flow::Ready(()) }
                    } else {
                        WaitAction { update: None, flow: Flow::Pending(()) }
                    }
                },
                |_| (),
                |_| self.unlock(),
                |_| (),
            ).await;
            MutexGuard { mutex: self }
        }
    }

    pub(crate) unsafe fn unlock(&self) {
        let mut waiters = self.futex.lock();
        //TODO: optimize for the non-contended case
        let mut queue = waiters.lock();
        if let Some(waiter) = queue.pop(0) {
            mem::drop(queue);
            waiter.wake();
            return;
        }
        let mut atom = self.futex.load(Relaxed);
        loop {
            if atom.has_new_waiters() {
                if queue.cmpxchg_enqueue_weak(&mut atom, 1usize, Acquire, Relaxed) {
                    let waiter = queue.pop(0).unwrap();
                    mem::drop(queue);
                    waiter.wake();
                    return;
                }
            } else {
                mem::drop(queue);
                if self.futex.cmpxchg_weak(&mut atom, 0usize, Release, Relaxed) {
                    return;
                } else {
                    queue = waiters.lock();
                }
            }
        }
    }
}

impl<'a, T> Drop for MutexGuard<'a, T> {
    fn drop(&mut self) {
        unsafe { self.mutex.unlock(); }
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