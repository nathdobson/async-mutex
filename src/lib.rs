#![allow(unused_imports)]
#![allow(unused_variables)]
#![allow(unused_mut)]
#![allow(dead_code)]
#![allow(unused_unsafe)]
#![deny(unused_must_use)]

#![feature(arbitrary_self_types)]
#![feature(unboxed_closures)]
#![feature(once_cell)]
#![feature(fn_traits)]
#![feature(negative_impls)]
#![feature(integer_atomics)]
#![feature(cfg_target_has_atomic)]
#![feature(backtrace)]
#![feature(stmt_expr_attributes)]

use crate::future::Future;
use std::task::{Context, Poll, RawWaker, RawWakerVTable};
use std::pin::Pin;
use crate::sync::atomic::AtomicUsize;
use crate::sync::atomic::Ordering::{Relaxed, Acquire, Release, AcqRel};
use crate::sync::Arc;
use std::mem;
use crate::atomic::{Atomic, Packable};
use std::task::{Waker};
use std::cell::{UnsafeCell, Cell};
use std::sync::{Weak};
use std::collections::HashMap;
pub(crate) use crate::loom::*;
use crate::sync::atomic::AtomicBool;
use std::ptr::{null, null_mut};
use std::lazy::SyncOnceCell;
use std::marker::{PhantomData};
use std::mem::{MaybeUninit, size_of, align_of};
use std::ops::{Deref, DerefMut};
use std::borrow::Borrow;
use crate::util::{AsyncFnOnce, FnOnceExt, Bind};
use crate::atomic_impl::{AtomicUsize2, usize2};
use std::process::abort;
use std::backtrace::{Backtrace, BacktraceStatus};
use crate::state::{MutexState, WaiterWaker, MutexWaker, CopyWaker};
use crate::waiter::Waiter;
use crate::cancel::Cancel;

mod atomic;
mod loom;
mod util;
mod atomic_impl;
#[cfg(test)]
mod test_waker;
#[cfg(test)]
mod tests;
mod state;
mod waiter;
pub mod cancel;

pub struct Mutex<T> {
    state: Atomic<MutexState>,
    owner_waker: Atomic<MutexWaker>,
    head: UnsafeCell<*const Waiter>,
    middle: UnsafeCell<*const Waiter>,
    inner: UnsafeCell<T>,
}

pub struct MutexGuard<'a, T> {
    scope: &'a MutexScope<'a, T>,
}

pub struct MutexScope<'a, T> {
    mutex: &'a Mutex<T>,
    scope_locked: Atomic<bool>,
    scope_active: UnsafeCell<bool>,
    mutex_held: UnsafeCell<bool>,
}

// enum CallbackState<'a, T, F: AsyncFnOnce<(&'a MutexScope<'a, T>, )>> {
//     Enter(F),
//     Running(F::Output),
//     Done,
// }

#[must_use = "with does nothing unless polled/`await`-ed"]
pub struct UncheckedWithFuture<'a, T, F: Future> {
    scope: &'a MutexScope<'a, T>,
    inner: F,
}

#[derive(Clone, Copy)]
enum LockFutureStep {
    Enter,
    Waiting { canceling: bool },
    Done,
    Canceled,
}

#[must_use = "with does nothing unless polled/`await`-ed"]
pub struct LockFuture<'a, T> {
    cancel: &'a Cancel,
    scope: &'a MutexScope<'a, T>,
    step: LockFutureStep,
    waiter: Waiter,
}

impl<T> Mutex<T> {
    pub fn new(inner: T) -> Self {
        Mutex {
            state: Atomic::new(MutexState {
                locked: false,
                tail: null(),
                canceling: null(),
            }),
            head: UnsafeCell::new(null()),
            middle: UnsafeCell::new(null()),
            inner: UnsafeCell::new(inner),
            owner_waker: Atomic::new(MutexWaker::None),
        }
    }

    pub fn get_mut(&mut self) -> &mut T {
        self.inner.get_mut()
    }

    pub fn scope<'a>(&'a self) -> MutexScope<'a, T> {
        MutexScope {
            mutex: self,
            scope_locked: Atomic::new(false),
            scope_active: UnsafeCell::new(false),
            mutex_held: UnsafeCell::new(false),
        }
    }

    unsafe fn normalize(&self, unlock: bool) {
        let mut state = self.state.load(Acquire);
        loop {
            let mut new_middle = state.tail;
            let mut canceling = state.canceling;
            let head = self.head.get();
            if new_middle != null() || canceling != null() {
                let new_state = MutexState {
                    locked: true,
                    tail: null(),
                    canceling: null(),
                };
                if !self.state.cmpxchg_weak(&mut state, new_state, Acquire, Acquire) {
                    continue;
                }
                assert!(state.locked);
                let walk = new_middle;
                while walk != null_mut() {
                    if *(*walk).prev.get() == null() {
                        break;
                    } else {
                        *(**(*walk).prev.get()).next.get() = walk;
                    }
                }
                *self.middle.get() = new_middle;
                if *self.head.get() == null() {
                    *self.head.get() = walk;
                }
                while canceling != null() {
                    if *(*canceling).prev.get() == null() {
                        *self.head.get() = *(*canceling).next.get();
                    } else {
                        *(**(*canceling).prev.get()).next.get() = *(*canceling).next.get();
                    }
                    if *(*canceling).next.get() == null() {
                        *self.middle.get() = *(*canceling).prev.get();
                    } else {
                        *(**(*canceling).next.get()).prev.get() = *(*canceling).prev.get();
                    }
                    let next_canceling = *(*canceling).next_canceling.get();
                    let woken = mem::replace(&mut canceling, next_canceling);
                    match (*woken).waker.swap(WaiterWaker::Canceled, Acquire) {
                        WaiterWaker::Waiting(waker) => {
                            waker.into_waker().wake();
                        }
                        _ => unreachable!(),
                    }
                }
                continue;
            } else if !unlock {
                return;
            } else if *head == null() {
                let new_state = MutexState {
                    locked: false,
                    tail: null(),
                    canceling: null(),
                };
                if self.state.cmpxchg_weak(&mut state, new_state, AcqRel, Acquire) {
                    return;
                }
            } else {
                let woken = mem::replace(&mut *head, *(**head).next.get());
                match (*woken).waker.swap(WaiterWaker::Locked, Acquire) {
                    WaiterWaker::Waiting(waker) => {
                        waker.into_waker().wake();
                    }
                    _ => unreachable!(),
                }
                return;
            }
        }
    }
    // unsafe fn unlock(&self) {
    //     if self.normalize(true).locked{
    //         let head = self.head.get();
    //         if *head != null() {
    //             let woken = mem::replace(&mut *head, *(**head).next.get());
    //             match (*woken).waker.swap(WaiterWaker::Locked, Acquire) {
    //                 WaiterWaker::Waiting(waker) => {
    //                     waker.into_waker().wake();
    //                 }
    //                 _ => unreachable!(),
    //             }
    //         }
    //     }
    // }
}

impl<'a, T> MutexScope<'a, T> {
    fn scope_lock(&self) {
        let mut state = false;
        if !self.scope_locked.cmpxchg(&mut state, true, Acquire, Relaxed) {
            panic!("MutexScope shared across threads");
        }
    }
    fn scope_unlock(&self) {
        self.scope_locked.store(false, Release);
    }
    pub fn with<F>(&'a self, fut: F) -> UncheckedWithFuture<'a, T, F> where F: Future {
        UncheckedWithFuture {
            scope: self,
            inner: fut,
        }
    }
    pub fn lock(&'a self, cancel:&'a Cancel) -> LockFuture<'a, T> {
        LockFuture {
            cancel,
            scope: self,
            step: LockFutureStep::Enter,
            waiter: Waiter::new(),
        }
    }
}

impl<'a, T> LockFuture<'a, T> {
    unsafe fn poll_enter(&mut self, cx: &mut Context<'_>) -> Poll<MutexGuard<'a, T>> {
        if self.cancel.cancelling() {
            self.step = LockFutureStep::Canceled;
            return Poll::Pending;
        }
        let mut state = self.scope.mutex.state.load(Relaxed);
        //TODO: Don't do this in the fast path
        let waker = CopyWaker::from_waker(cx.waker().clone());
        self.waiter.waker.store_mut(WaiterWaker::Waiting(waker));
        loop {
            if !state.locked {
                assert_eq!(state.tail, null());
                assert_eq!(state.canceling, null());
                let new_state = MutexState { locked: true, ..state };
                if self.scope.mutex.state.cmpxchg_weak(&mut state, new_state, Acquire, Relaxed) {
                    mem::drop(waker.into_waker());
                    self.waiter.waker.store_mut(WaiterWaker::None);
                    self.step = LockFutureStep::Done;
                    return Poll::Ready(MutexGuard::new(self.scope));
                }
            } else {
                *self.waiter.prev.get_mut() = state.tail;
                let new_state = MutexState { tail: &self.waiter, ..state };
                if self.scope.mutex.state.cmpxchg_weak(&mut state, new_state, Release, Relaxed) {
                    self.step = LockFutureStep::Waiting { canceling: false };
                    return Poll::Pending;
                }
            }
        }
    }
    unsafe fn poll_waiting(&mut self, cx: &mut Context<'_>, canceling: bool) -> Poll<MutexGuard<'a, T>> {
        let mut old_waker = self.waiter.waker.load(Acquire);
        //TODO: Don't do this in the fast path
        let new_waker = CopyWaker::from_waker(cx.waker().clone());
        loop {
            match old_waker {
                WaiterWaker::Waiting(old_waker_value) => {
                    mem::drop(old_waker_value.into_waker());
                    if self.waiter.waker.cmpxchg_weak(&mut old_waker, WaiterWaker::Waiting(new_waker), AcqRel, Acquire) {
                        if self.cancel.cancelling() {
                            if !canceling {
                                self.step = LockFutureStep::Waiting { canceling: true };
                                let mut state = self.scope.mutex.state.load(Relaxed);
                                loop {
                                    *self.waiter.next_canceling.get_mut() = state.canceling;
                                    let new_state = MutexState { canceling: &self.waiter, ..state };
                                    if self.scope.mutex.state.cmpxchg_weak(&mut state, new_state, Release, Relaxed) {
                                        break;
                                    }
                                }
                            }
                        }
                        return Poll::Pending;
                    }
                }
                WaiterWaker::Locked => {
                    mem::drop(new_waker.into_waker());
                    self.step = LockFutureStep::Done;
                    return Poll::Ready(MutexGuard::new(self.scope));
                }
                WaiterWaker::Canceled => {
                    assert!(canceling);
                    self.cancel.set_cancelled();
                    mem::drop(new_waker.into_waker());
                    self.step = LockFutureStep::Canceled;
                    return Poll::Pending;
                }
                WaiterWaker::None => panic!(),
            }
        }
    }
}

impl<'a, T, F: Future> Future for UncheckedWithFuture<'a, T, F> {
    type Output = F::Output;
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        unsafe {
            let this = self.get_unchecked_mut();
            //TODO: do this once
            this.scope.scope_lock();
            *this.scope.scope_active.get() = true;
            this.scope.scope_unlock();
            //TODO: handle inner panic
            let result = Pin::new_unchecked(&mut this.inner).poll(cx);
            this.scope.scope_lock();
            if result.is_ready() {
                assert!(!*this.scope.mutex_held.get());
                *this.scope.scope_active.get() = false;
            } else if *this.scope.mutex_held.get() {
                match this.scope.mutex.owner_waker.swap(MutexWaker::Waiting(CopyWaker::from_waker(cx.waker().clone())), AcqRel) {
                    MutexWaker::Waiting(old) => {
                        mem::drop(old);
                    }
                    _ => {}
                }
                this.scope.mutex.normalize(false);
            }
            this.scope.scope_unlock();
            result
        }
    }
}

impl<'a, T> Future for LockFuture<'a, T> {
    type Output = MutexGuard<'a, T>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        unsafe {
            let this = self.get_unchecked_mut();
            match this.step {
                LockFutureStep::Enter => this.poll_enter(cx),
                LockFutureStep::Waiting { canceling } => this.poll_waiting(cx, canceling),
                LockFutureStep::Done => panic!("Future already completed!"),
                LockFutureStep::Canceled => return Poll::Pending,
            }
        }
    }
}

impl<'a, T> Drop for LockFuture<'a, T> {
    fn drop(&mut self) {
        unsafe {
            match self.step {
                LockFutureStep::Enter => {}
                LockFutureStep::Waiting { canceling: bool } => bad_cancel(),
                LockFutureStep::Done => {}
                LockFutureStep::Canceled => {}
            }
        }
    }
}

impl<'a, T> MutexGuard<'a, T> {
    unsafe fn new(scope: &'a MutexScope<'a, T>) -> Self {
        scope.scope_lock();
        assert!(*scope.scope_active.get());
        assert!(scope.mutex.state.load(Relaxed).locked);
        *scope.mutex_held.get() = true;
        scope.scope_unlock();
        MutexGuard { scope }
    }
}

impl<'a, T> Drop for MutexGuard<'a, T> {
    fn drop(&mut self) {
        unsafe {
            self.scope.scope_lock();
            assert!(*self.scope.scope_active.get());
            *self.scope.mutex_held.get() = false;
            assert!(self.scope.mutex.state.load(Relaxed).locked);
            self.scope.mutex.normalize(true);
            match self.scope.mutex.owner_waker.swap(MutexWaker::None, Acquire) {
                MutexWaker::Waiting(waker) => mem::drop(waker.into_waker()),
                _ => {}
            };
            self.scope.scope_unlock();
        }
    }
}

fn bad_cancel() -> ! {
    eprintln!("Attempted to drop future before canceling completed: aborting.");
    let bt = Backtrace::capture();
    if bt.status() == BacktraceStatus::Captured {
        eprintln!("{}", bt);
    }
    abort();
}

unsafe impl<'a, T: Send> Send for LockFuture<'a, T> {}

unsafe impl<T> Send for Mutex<T> {}

unsafe impl<T> Sync for Mutex<T> {}

unsafe impl<'a, T> Send for MutexGuard<'a, T> {}

unsafe impl<'a, T> Sync for MutexGuard<'a, T> {}

unsafe impl<'a, T> Sync for MutexScope<'a, T> {}

unsafe impl<'a, T> Send for MutexScope<'a, T> {}

unsafe impl<'a, T, F: Future> Send for UncheckedWithFuture<'a, T, F> {}

impl<'a, T> Deref for MutexGuard<'a, T> {
    type Target = T;
    fn deref(&self) -> &Self::Target { unsafe { &*self.scope.mutex.inner.get() } }
}

impl<'a, T> DerefMut for MutexGuard<'a, T> {
    fn deref_mut(&mut self) -> &mut Self::Target { unsafe { &mut *self.scope.mutex.inner.get() } }
}