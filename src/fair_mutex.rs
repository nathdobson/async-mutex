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

use crate::futex::atomic::{Atomic, Packable};
use crate::futex::atomic_impl::{AtomicUsize2, usize2};
use crate::cell::UnsafeCell;
use crate::futex::{EnterResult, Futex, WaitFuture};
use crate::futex::WaitImpl;
use crate::future::Future;
use crate::sync::Arc;
use crate::sync::atomic::AtomicBool;
use crate::sync::atomic::AtomicUsize;
use crate::sync::atomic::Ordering::{AcqRel, Acquire, Relaxed, Release};
use crate::test_println;
use crate::util::{AsyncFnOnce, bad_cancel, Bind, FnOnceExt};
use crate::futex::waiter::Waiter;

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
        Mutex { futex: Futex::new(0), inner: UnsafeCell::new(inner) }
    }

    pub fn get_mut(&mut self) -> &mut T {
        unsafe { self.inner.with_mut(|x| &mut *x) }
    }

    pub async fn lock<'a>(&'a self) -> MutexGuard<'a, T> {
        unsafe {
            let mut flipped = false;
            let guard = self.futex.wait(WaitImpl {
                enter: |locked| if locked == 0 {
                    flipped = true;
                    EnterResult { userdata: Some(1), pending: false }
                } else {
                    flipped = false;
                    EnterResult { userdata: None, pending: true }
                },
                post_enter: Some(|| {}),
                exit: || MutexGuard { mutex: self },
                phantom: PhantomData::<usize>,
            }).await;
            if flipped {
                test_println!("Flipped on");
            }
            guard
        }
    }
    pub(crate) unsafe fn unlock(&self) {
        test_println!("Unlocking");
        let state = self.futex.lock_state();
        let mut flipped = false;
        let waiter = state.with_mut(|state| {
            self.futex.update_flip(&mut *state, |atom, queued| {
                flipped = !queued;
                (!queued).then_some(0)
            });
            self.futex.pop(&mut *state)
        });
        assert!(flipped != waiter.is_some());
        if let Some(waiter) = waiter {
            test_println!("Waiter is done");
            waiter.done();
        } else {
            test_println!("Flipped off");
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