use crate::waiter::Waiter;
use crate::atomic::Atomic;
use std::ptr::null;
use std::mem::MaybeUninit;
use std::mem;
use crate::cell::UnsafeCell;
use crate::future::Future;
use std::task::{Context, Poll};
use std::pin::Pin;
use crate::state::{CopyWaker, WaiterWaker};
use async_cancel::Cancel;
use crate::sync::atomic::Ordering::Acquire;
use crate::sync::atomic::Ordering::AcqRel;

pub struct Condvar {
    head: UnsafeCell<*const Waiter>,
    tail: UnsafeCell<*const Waiter>,
}

enum WaitFutureStep {
    Enter,
    Waiting { canceling: bool },
    Done,
    Canceled,
}

pub struct WaitFuture<'c, 'm, T> {
    waiter: MaybeUninit<Waiter>,
    cancel: &'c Cancel,
    condvar: &'c Condvar,
    step: WaitFutureStep,

}

impl Condvar {
    pub fn new() -> Self {
        Condvar { head: UnsafeCell::new(null()), tail: UnsafeCell::new(null()) }
    }
    pub fn wait<'c, 'm, T>(&'c self, cancel: &'c Cancel, guard: &'m mut MutexGuard<'m, T>) -> WaitFuture<'c, 'm, T> {
        let scope = guard.scope;
        WaitFuture {
            waiter: MaybeUninit::new(Waiter::new()),
            cancel,
            condvar: self,
            scope,
            step: WaitFutureStep::Enter,
        }
    }
    pub fn notify<T>(&self, guard: &mut MutexGuard<T>, number: usize) {
        todo!()
    }
    unsafe fn head(&self) -> *const Waiter { self.head.with_mut(|x| *x) }
    unsafe fn tail(&self) -> *const Waiter { self.tail.with_mut(|x| *x) }
    unsafe fn set_head(&self, head: *const Waiter) {
        self.head.with_mut(|x| *x = head)
    }
    unsafe fn set_tail(&self, head: *const Waiter) {
        self.tail.with_mut(|x| *x = head)
    }
}

impl<'c, 'm, T> WaitFuture<'c, 'm, T> {
    unsafe fn poll_enter(&mut self, cx: &mut Context<'_>) -> Poll<()> {
        if self.cancel.canceling() {
            self.step = WaitFutureStep::Canceled;
            return Poll::Pending;
        }
        let waker = CopyWaker::from_waker(cx.waker().clone());
        self.waiter.as_mut_ptr().waker_mut().store_mut(WaiterWaker::Waiting { waker, canceling: false });
        self.scope.scope_lock(|state| {
            let tail = self.condvar.tail();
            if tail != null() {
                self.waiter.as_ptr().set_prev(self.condvar.tail());
                self.condvar.tail().set_next(self.waiter.as_ptr());
            }
            self.condvar.set_tail(self.waiter.as_ptr());
            self.scope.do_release(state);
        });
        Poll::Pending
    }
    unsafe fn poll_waiting(&mut self, cx: &mut Context<'_>, canceling: bool) -> Poll<()> {
        let mut old_waker = self.waiter.as_ptr().waker().load(Acquire);
        let new_waker = CopyWaker::from_waker(cx.waker().clone());
        let new_canceling = self.cancel.canceling();
        loop {
            match old_waker {
                WaiterWaker::Waiting { waker: old_waker_value, .. } => {
                    if self.waiter.as_ptr().waker().cmpxchg_weak(
                        &mut old_waker, WaiterWaker::Waiting { waker: new_waker, canceling: new_canceling }, AcqRel, Acquire) {
                        mem::drop(old_waker_value.into_waker());
                        if new_canceling {
                            self.cancel.set_pending();
                            if !canceling {
                                //self.cancel();
                                todo!()
                            }
                        }
                        return Poll::Pending;
                    }
                }
                WaiterWaker::Done { canceled: false } => {
                    mem::drop(new_waker.into_waker());
                    self.step = WaitFutureStep::Done;
                    self.scope.scope_lock(|state|{
                        self.scope.on_acquire(state)
                    });
                    return Poll::Ready(());
                }
                WaiterWaker::Done { canceled: true } => {
                    assert!(canceling);
                    mem::drop(new_waker.into_waker());
                    self.step = WaitFutureStep::Canceled;
                    return Poll::Pending;
                }
                WaiterWaker::None => panic!(),
            }
        }
    }
}

impl<'c, 'm, T> Future for WaitFuture<'c, 'm, T> {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        unsafe {
            let this = self.get_unchecked_mut();
            match this.step {
                WaitFutureStep::Enter => this.poll_enter(cx),
                WaitFutureStep::Waiting { canceling } => this.poll_waiting(cx, canceling),
                WaitFutureStep::Done => panic!(),
                WaitFutureStep::Canceled => Poll::Pending,
            }
        }
    }
}

impl<'c, 'm, T> Drop for WaitFuture<'c, 'm, T> {
    fn drop(&mut self) {
        match self.step {
            WaitFutureStep::Enter => {}
            WaitFutureStep::Waiting { .. } => panic!(),
            WaitFutureStep::Done => {}
            WaitFutureStep::Canceled => {}
        }
    }
}
