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
    futex: Futex,
    inner: UnsafeCell<T>,
}

#[derive(Debug)]
pub struct MutexGuard<'a, T> {
    pub(crate) mutex: &'a Mutex<T>,
}

const UNLOCKED: usize = 0;
const UNLOCKED_WAKING: usize = 1;
const LOCKED: usize = 2;
const LOCKED_WAKING: usize = 3;

impl<T> Mutex<T> {
    pub fn new(inner: T) -> Self {
        Mutex { futex: Futex::new(0), inner: UnsafeCell::new(inner) }
    }

    pub fn get_mut(&mut self) -> &mut T {
        unsafe { self.inner.with_mut(|x| &mut *x) }
    }

    pub async fn lock<'a>(&'a self) -> MutexGuard<'a, T> {
        unsafe {
            struct OnDrop;
            impl Drop for OnDrop { fn drop(&mut self) { todo!() } }
            let drop = OnDrop;

            let mut locked = false;
            self.futex.wait(WaitImpl {
                enter: |userdata| match userdata {
                    UNLOCKED => {
                        locked = true;
                        test_println!("UNLOCKED -> LOCKED");
                        EnterResult { userdata: Some(LOCKED), pending: false }
                    }
                    UNLOCKED_WAKING | LOCKED | LOCKED_WAKING => {
                        locked = false;
                        test_println!("UNLOCKED_WAKING | LOCKED | LOCKED_WAKING");
                        EnterResult { userdata: None, pending: true }
                    }
                    _ => panic!(),
                },
                post_enter: Some(|| {}),
                exit: || (),
                phantom: PhantomData,
            }).await;
            while !locked {
                self.futex.wait(WaitImpl {
                    enter: |userdata| match userdata {
                        LOCKED_WAKING => {
                            test_println!("LOCKED_WAKING -> LOCKED");
                            locked = false;
                            EnterResult { userdata: Some(LOCKED), pending: true }
                        }
                        UNLOCKED_WAKING => {
                            test_println!("UNLOCKED_WAKING -> LOCKED");
                            locked = true;
                            EnterResult { userdata: Some(LOCKED), pending: false }
                        }
                        _ => panic!(),
                    },
                    post_enter: Some(|| {}),
                    exit: || (),
                    phantom: PhantomData,
                }).await;
            }

            mem::forget(drop);
            MutexGuard { mutex: self }
        }
    }
}

impl<'a, T> Drop for MutexGuard<'a, T> {
    fn drop(&mut self) {
        unsafe {
            test_println!("Unlocking");
            let state = self.mutex.futex.lock_state();
            let waiter = state.with_mut(|state| {
                let mut pop = false;
                self.mutex.futex.update_flip(&mut *state, |atom, queued| {
                    match atom {
                        LOCKED if queued => {
                            test_println!("LOCKED -> UNLOCKED_WAKING");
                            pop = true;
                            Some(UNLOCKED_WAKING)
                        }
                        LOCKED if !queued => {
                            test_println!("LOCKED -> UNLOCKED");
                            pop = false;
                            Some(UNLOCKED)
                        }
                        LOCKED_WAKING => {
                            test_println!("LOCKED_WAKING -> UNLOCKED_WAKING");
                            pop = false;
                            Some(UNLOCKED_WAKING)
                        }
                        UNLOCKED => panic!(),
                        UNLOCKED_WAKING => panic!(),
                        _ => panic!(),
                    }
                });
                if pop {
                    test_println!("popping");
                    self.mutex.futex.pop(&mut *state)
                } else {
                    None
                }
            });
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