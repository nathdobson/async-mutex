use crate::future::Future;
use std::task::{Context, Poll, RawWaker, RawWakerVTable};
use std::pin::Pin;
use crate::sync::atomic::AtomicUsize;
use crate::sync::atomic::Ordering::{Relaxed, Acquire, Release, AcqRel};
use crate::sync::Arc;
use std::mem;
use crate::atomic::{Atomic, Packable};
use std::task::{Waker};
use crate::cell::{UnsafeCell};
use std::sync::{Weak};
use std::collections::HashMap;
use crate::sync::atomic::AtomicBool;
use std::ptr::{null, null_mut};
use std::lazy::SyncOnceCell;
use std::marker::{PhantomData};
use std::mem::{MaybeUninit, size_of, align_of};
use std::ops::{Deref, DerefMut};
use std::borrow::Borrow;
use crate::util::{AsyncFnOnce, FnOnceExt, Bind, bad_cancel};
use crate::atomic_impl::{AtomicUsize2, usize2};
use std::process::abort;
use std::backtrace::{Backtrace, BacktraceStatus};
use crate::waiter::Waiter;
use crate::test_println;
use crate::futex::{Futex, WaitFuture, EnterResult};
use crate::futex::WaitImpl;

#[derive(Debug)]
pub struct Mutex<T> {
    futex: Futex,
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
            // if self.fair {
                self.futex.wait(WaitImpl {
                    enter: |userdata| if userdata == 0 {
                        EnterResult { userdata: Some(1), pending: false }
                    } else {
                        EnterResult { userdata: None, pending: true }
                    },
                    exit: || MutexGuard { mutex: self },
                }).await
            // } else {
            //     let mut locked = false;
            //     while !locked {
            //         self.futex.wait(WaitImpl {
            //             enter: |userdata| if userdata == 0 {
            //                 locked = true;
            //                 EnterResult { userdata: Some(1), pending: false }
            //             } else {
            //                 EnterResult { userdata: None, pending: true }
            //             },
            //             exit: || (),
            //         });
            //     }
            // }
        }
    }
}

impl<'a, T> Drop for MutexGuard<'a, T> {
    fn drop(&mut self) {
        unsafe {
            let state = self.mutex.futex.lock_state();
            let waiter = state.with_mut(|state|
                self.mutex.futex.pop_or_update(&mut *state, |locked| Some(0))
            );
            if let Some(waiter) = waiter {
                test_println!("Waiter is done");
                waiter.done();
            }
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