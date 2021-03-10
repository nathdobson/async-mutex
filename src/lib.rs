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
#![feature(raw_ref_op)]
#![feature(future_poll_fn)]
#![feature(test)]

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

#[cfg(test)]
extern crate test;

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

#[derive(Debug)]
struct MutexQueue {
    head: *const Waiter,
    middle: *const Waiter,
}

#[derive(Debug)]
pub struct Mutex<T> {
    state: Atomic<MutexState>,
    owner_waker: Atomic<MutexWaker>,
    queue: UnsafeCell<MutexQueue>,
    inner: UnsafeCell<T>,
}

#[derive(Debug)]
pub struct MutexGuard<'a, T> {
    scope: &'a MutexScope<'a, T>,
}

#[derive(Debug)]
struct MutexScopeState {
    scope_active: bool,
    mutex_held: bool,
}

#[derive(Debug)]
pub struct MutexScope<'a, T> {
    mutex: &'a Mutex<T>,
    scope_locked: Atomic<bool>,
    state: UnsafeCell<MutexScopeState>,
}

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
    // TODO MaybeUninit isn't exactly right here: the wrapper should communicate to
    // the compiler that it is unsafe to convert a `&mut LockFuture` to a `&mut Waiter`,
    // while it is safe to convert an `&mut LockFuture` to a `&Waiter`.
    waiter: MaybeUninit<Waiter>,
}

impl<T> Mutex<T> {
    pub fn new(inner: T) -> Self {
        Mutex {
            state: Atomic::new(MutexState {
                locked: false,
                tail: null(),
                canceling: null(),
            }),
            queue: UnsafeCell::new(MutexQueue {
                head: null(),
                middle: null(),
            }),
            inner: UnsafeCell::new(inner),
            owner_waker: Atomic::new(MutexWaker::None),
        }
    }

    pub fn get_mut(&mut self) -> &mut T {
        unsafe {
            self.inner.with_mut(|x| &mut *x)
        }
    }

    pub fn scope<'a>(&'a self) -> MutexScope<'a, T> {
        MutexScope {
            mutex: self,
            scope_locked: Atomic::new(false),
            state: UnsafeCell::new(MutexScopeState {
                scope_active: false,
                mutex_held: false,
            }),
        }
    }

    unsafe fn head(&self) -> *const Waiter {
        self.queue.with_mut(|queue| (*queue).head)
    }
    unsafe fn middle(&self) -> *const Waiter {
        self.queue.with_mut(|queue| (*queue).middle)
    }
    unsafe fn set_head(&self, head: *const Waiter) {
        self.queue.with_mut(|queue| (*queue).head = head);
    }
    unsafe fn set_middle(&self, middle: *const Waiter) {
        self.queue.with_mut(|queue| (*queue).middle = middle);
    }
    // unsafe fn validate_queue(&self) {
    //     test_println!("queue= ");
    //     let mut waiter = self.head();
    //     if waiter != null() {
    //         assert_eq!(waiter.prev(), null());
    //     }
    //     while waiter != null() {
    //         test_println!("queue= {:?}", waiter);
    //         if waiter.next() == null() {
    //             assert_eq!(self.middle(), waiter);
    //         } else {
    //             assert_eq!(waiter.next().prev(), waiter);
    //         }
    //         waiter = waiter.next();
    //     }
    // }
    unsafe fn normalize(&self, unlock: bool) {
        test_println!("normalize {}", unlock);
        let mut state = self.state.load(Acquire);
        loop {
            let mut new_middle = state.tail;
            let mut canceling = state.canceling;
            if new_middle != null() || canceling != null() {
                test_println!("normalize: queueing {:?} {:?}", new_middle, canceling);
                let new_state = MutexState {
                    locked: true,
                    tail: null(),
                    canceling: null(),
                };
                if !self.state.cmpxchg_weak(&mut state, new_state, AcqRel, Acquire) {
                    continue;
                }
                assert!(state.locked);
                if new_middle != null() {
                    let mut walk = new_middle;
                    loop {
                        let prev = walk.prev();
                        if prev == null() {
                            break;
                        } else {
                            prev.set_next(walk);
                            walk = prev;
                        }
                    }
                    assert!(walk != null());
                    let old_middle = self.middle();
                    if old_middle != null() {
                        old_middle.set_next(walk);
                    }
                    if walk != null() {
                        walk.set_prev(old_middle);
                    }
                    self.set_middle(new_middle);
                    if self.head() == null() {
                        self.set_head(walk);
                    }
                }
                test_println!("canceling loop:");
                while canceling != null() {
                    if canceling.prev() == null() {
                        if self.head() == canceling {
                            self.set_head(canceling.next());
                        }
                    } else {
                        canceling.prev().set_next(canceling.next());
                    }
                    if canceling.next() == null() {
                        if self.middle() == canceling {
                            self.set_middle(canceling.prev());
                        }
                    } else {
                        canceling.next().set_prev(canceling.prev());
                    }
                    let woken = canceling;
                    test_println!("Canceling {:?}", woken);
                    canceling = canceling.next_canceling();
                    let mut waker = woken.waker().load(Acquire);
                    loop {
                        match waker {
                            WaiterWaker::Waiting { waker: waker_value, canceling: true } => {
                                if woken.waker().cmpxchg_weak(&mut waker, WaiterWaker::Done { canceled: true }, AcqRel, Acquire) {
                                    waker_value.into_waker().wake();
                                    break;
                                }
                            }
                            WaiterWaker::Done { canceled: false } => break,
                            _ => panic!("{:?}", waker),
                        }
                    }
                }
                state = new_state;
                continue;
            } else if !unlock {
                test_println!("normalize: locked");
                return;
            } else if self.queue.with_mut(|x| (*x).head) == null() {
                test_println!("normalize: queue empty, unlocking");
                let new_state = MutexState {
                    locked: false,
                    tail: null(),
                    canceling: null(),
                };
                if self.state.cmpxchg_weak(&mut state, new_state, AcqRel, Acquire) {
                    return;
                }
            } else {
                test_println!("normalize: queue present, waking");
                let woken = self.head();
                let new_head = woken.next();
                self.set_head(new_head);
                if new_head != null() {
                    new_head.set_prev(null());
                }
                if self.middle() == woken {
                    self.set_middle(null());
                }
                // even in canceling, the operation will succeed.
                // TODO: improve performance by canceling this waiter and moving on now.
                match woken.waker().swap(WaiterWaker::Done { canceled: false }, AcqRel) {
                    WaiterWaker::Waiting { waker, canceling } => {
                        waker.into_waker().wake();
                    }
                    _ => unreachable!(),
                }
                return;
            }
        }
    }
}

impl<'a, T> MutexScope<'a, T> {
    unsafe fn scope_lock<R>(&self, f: impl FnOnce(&mut MutexScopeState) -> R) -> R {
        let mut state = false;
        if !self.scope_locked.cmpxchg(&mut state, true, AcqRel, Acquire) {
            panic!("MutexScope shared across threads");
        }
        let r = self.state.with_mut(|s| f(&mut *s));
        self.scope_locked.store(false, Release);
        r
    }
    pub fn with<F>(&'a self, fut: F) -> UncheckedWithFuture<'a, T, F> where F: Future {
        UncheckedWithFuture {
            scope: self,
            inner: fut,
        }
    }
    pub fn lock(&'a self, cancel: &'a Cancel) -> LockFuture<'a, T> {
        LockFuture {
            cancel,
            scope: self,
            step: LockFutureStep::Enter,
            waiter: MaybeUninit::new(Waiter::new()),
        }
    }
}

impl<'a, T> LockFuture<'a, T> {
    unsafe fn poll_enter(&mut self, cx: &mut Context<'_>) -> Poll<MutexGuard<'a, T>> {
        test_println!("poll_enter");
        if self.cancel.cancelling() {
            self.step = LockFutureStep::Canceled;
            test_println!("poll_enter: canceled");
            return Poll::Pending;
        }
        let mut state = self.scope.mutex.state.load(Acquire);
        //TODO: Don't do this in the fast path
        test_println!("poll_enter: cloning");
        let waker = CopyWaker::from_waker(cx.waker().clone());
        self.waiter.as_mut_ptr().waker_mut().store_mut(WaiterWaker::Waiting { waker, canceling: false });
        loop {
            if !state.locked {
                assert_eq!(state.tail, null());
                assert_eq!(state.canceling, null());
                let new_state = MutexState { locked: true, ..state };
                if self.scope.mutex.state.cmpxchg_weak(&mut state, new_state, AcqRel, Acquire) {
                    test_println!("poll_enter: dropping");
                    mem::drop(waker.into_waker());
                    self.waiter.as_mut_ptr().waker_mut().store_mut(WaiterWaker::None);
                    self.step = LockFutureStep::Done;
                    test_println!("poll_enter: locked");
                    return Poll::Ready(MutexGuard::new(self.scope));
                }
            } else {
                self.waiter.as_ptr().set_prev(state.tail);
                let new_state = MutexState { tail: self.waiter.as_ptr(), ..state };
                if self.scope.mutex.state.cmpxchg_weak(&mut state, new_state, AcqRel, Acquire) {
                    self.step = LockFutureStep::Waiting { canceling: false };
                    test_println!("poll_enter: waiting {:?}", self.waiter.as_ptr());
                    return Poll::Pending;
                }
            }
        }
    }
    unsafe fn cancel(&mut self) {
        self.step = LockFutureStep::Waiting { canceling: true };
        let mut state = self.scope.mutex.state.load(Acquire);
        loop {
            self.waiter.as_ptr().set_next_canceling(state.canceling);
            let new_state = MutexState { canceling: self.waiter.as_ptr(), ..state };
            if self.scope.mutex.state.cmpxchg_weak(&mut state, new_state, AcqRel, Acquire) {
                break;
            }
        }
        match self.scope.mutex.owner_waker.swap(MutexWaker::Canceling, AcqRel) {
            MutexWaker::None => {}
            MutexWaker::Canceling => {}
            MutexWaker::Waiting(waker) => waker.into_waker().wake(),
        }
    }
    unsafe fn poll_waiting(&mut self, cx: &mut Context<'_>, canceling: bool) -> Poll<MutexGuard<'a, T>> {
        test_println!("poll_waiting");
        let mut old_waker = self.waiter.as_ptr().waker().load(Acquire);
        //TODO: Don't do this in the fast path
        test_println!("poll_waiting: cloning");
        let new_waker = CopyWaker::from_waker(cx.waker().clone());
        let new_canceling = self.cancel.cancelling();
        loop {
            match old_waker {
                WaiterWaker::Waiting { waker: old_waker_value, .. } => {
                    if self.waiter.as_ptr().waker().cmpxchg_weak(
                        &mut old_waker, WaiterWaker::Waiting { waker: new_waker, canceling: new_canceling }, AcqRel, Acquire) {
                        test_println!("poll_waiting: dropping old");
                        mem::drop(old_waker_value.into_waker());
                        if new_canceling {
                            self.cancel.set_pending();
                            if !canceling {
                                self.cancel();
                            }
                        }
                        test_println!("poll_waiting: waiting");
                        return Poll::Pending;
                    }
                }
                WaiterWaker::Done { canceled: false } => {
                    test_println!("poll_waiting: dropping new");
                    mem::drop(new_waker.into_waker());
                    self.step = LockFutureStep::Done;
                    test_println!("poll_waiting: locked");
                    return Poll::Ready(MutexGuard::new(self.scope));
                }
                WaiterWaker::Done { canceled: true } => {
                    assert!(canceling);
                    test_println!("poll_waiting: dropping new");
                    mem::drop(new_waker.into_waker());
                    self.step = LockFutureStep::Canceled;
                    test_println!("poll_waiting: canceled");
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
            this.scope.scope_lock(|state| state.scope_active = true);
            //TODO: handle inner panic
            let result = Pin::new_unchecked(&mut this.inner).poll(cx);
            this.scope.scope_lock(|state| {
                if result.is_ready() {
                    assert!(!state.mutex_held);
                    state.scope_active = false;
                } else if state.mutex_held {
                    match this.scope.mutex.owner_waker.swap(MutexWaker::Waiting(CopyWaker::from_waker(cx.waker().clone())), AcqRel) {
                        MutexWaker::Waiting(old) => {
                            mem::drop(old);
                        }
                        _ => {}
                    }
                    this.scope.mutex.normalize(false);
                }
            });
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
        scope.scope_lock(|state| {
            assert!(state.scope_active);
            //assert!(scope.mutex.state.load(Relaxed).locked);
            state.mutex_held = true;
        });
        MutexGuard { scope }
    }
}

impl<'a, T> Drop for MutexGuard<'a, T> {
    fn drop(&mut self) {
        unsafe {
            self.scope.scope_lock(|state| {
                assert!(state.scope_active);
                state.mutex_held = false;
                //assert!(self.scope.mutex.state.load(Relaxed).locked);
                self.scope.mutex.normalize(true);
                match self.scope.mutex.owner_waker.swap(MutexWaker::None, AcqRel) {
                    MutexWaker::Waiting(waker) => mem::drop(waker.into_waker()),
                    _ => {}
                };
            });
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
    fn deref(&self) -> &Self::Target { unsafe { &*self.scope.mutex.inner.with_mut(|x| x) } }
}

impl<'a, T> DerefMut for MutexGuard<'a, T> {
    fn deref_mut(&mut self) -> &mut Self::Target { unsafe { &mut *self.scope.mutex.inner.with_mut(|x| x) } }
}