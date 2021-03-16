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

use crate::futex::atomic::{Atomic, Packable};
use crate::futex::atomic_impl::{AtomicUsize2, usize2};
use crate::cell::UnsafeCell;
use crate::futex::{Futex, WaitFuture, WaitAction, Flow};
use crate::future::Future;
use crate::sync::Arc;
use crate::sync::atomic::AtomicBool;
use crate::sync::atomic::AtomicUsize;
use crate::sync::atomic::Ordering::{AcqRel, Acquire, Relaxed, Release};
use crate::test_println;
use crate::util::{AsyncFnOnce, Bind, FnOnceExt};
use crate::futex::waiter::Waiter;
use core::time::Duration;
use crate::sync::atomic::Ordering::SeqCst;

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
        Mutex { futex: Futex::new(0, 1), inner: UnsafeCell::new(inner) }
    }

    pub fn get_mut(&mut self) -> &mut T {
        unsafe { self.inner.with_mut(|x| &mut *x) }
    }

    pub async fn lock<'a>(&'a self) -> MutexGuard<'a, T> {
        unsafe {
            self.futex.wait(
                0,
                0,
                |locked| {
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
        let lock = self.futex.lock();
        let mut is_queued = false;
        lock.update_flip(|atom, queued| {
            is_queued = queued;
            (!queued).then_some(0)
        });
        if is_queued {
            if let Some(waiter) = lock.pop(0) {
                waiter.done();
            }
        }
        mem::drop(lock);
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