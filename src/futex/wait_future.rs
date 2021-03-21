use crate::atomic::{Packable, IsReleaseT, IsLoadT, CopyWaker, AcqRelT, AcquireT};
use crate::futex::{Futex, FutexAtom, Waiter};
use std::marker::PhantomData;
use crate::futex::state::{WaiterWaker, RawFutexAtom};
use std::task::{Poll, Context};
use std::mem;
use std::fmt::Debug;
use crate::sync::atomic::Ordering::Acquire;
use crate::sync::atomic::Ordering::AcqRel;
use crate::sync::atomic::Ordering::Relaxed;
use crate::future::Future;
use std::pin::Pin;
use std::ptr::null;

#[derive(Clone, Copy)]
pub(in crate::futex) enum WaitFutureStep {
    Enter,
    Waiting,
    Done,
}

#[must_use = "with does nothing unless polled/`await`-ed"]
pub struct WaitFuture<'a, A, Success, Failure, OnPending, OnCancelWoken, OnCancelSleeping>
    where
        A: Packable<Raw=usize>,
        Success: IsReleaseT,
        Failure: IsLoadT,
        OnPending: FnOnce(),
        OnCancelWoken: FnOnce(),
        OnCancelSleeping: FnOnce(),
{
    pub(in crate::futex) original_futex: &'a Futex<A>,
    pub(in crate::futex) current: &'a mut FutexAtom<A>,
    pub(in crate::futex) new: A,
    pub(in crate::futex) step: WaitFutureStep,
    pub(in crate::futex) waiter: *const Waiter,
    pub(in crate::futex) on_pending: Option<OnPending>,
    pub(in crate::futex) on_cancel_woken: Option<OnCancelWoken>,
    pub(in crate::futex) on_cancel_sleeping: Option<OnCancelSleeping>,
    pub(in crate::futex) phantom: PhantomData<(A, Success, Failure)>,
}


impl<'a, A, Success, Failure, OnPending, OnCancelWoken, OnCancelSleeping>
WaitFuture<'a, A, Success, Failure, OnPending, OnCancelWoken, OnCancelSleeping>
    where
        A: Packable<Raw=usize> + Debug,
        Success: IsReleaseT,
        Failure: IsLoadT,
        OnPending: FnOnce(),
        OnCancelWoken: FnOnce(),
        OnCancelSleeping: FnOnce(),
{
    unsafe fn poll_enter(&mut self, cx: &mut Context) -> Poll<bool> {
        let mut waiter = self.waiter as *mut Waiter;
        (waiter as *const Waiter).set_prev(self.current.raw.inbox);
        match waiter.waker_mut().load_mut() {
            WaiterWaker::None => {
                let waker = CopyWaker::from_waker(cx.waker().clone());
                waiter.waker_mut().store_mut(WaiterWaker::Waiting { waker });
            }
            WaiterWaker::Waiting { .. } => {
                // Assume no awaits have occurred, so the old waker is valid.
            }
            WaiterWaker::Done => panic!("Reusing waiter"),
        }
        let new_futex_atom = RawFutexAtom { inbox: waiter, atom: A::encode(self.new) };
        // TODO: this allow `Release` instead of `AcqRel`.
        if self.original_futex.raw.cmpxchg_weak::<A, _, _>(&mut self.current.raw, new_futex_atom, AcqRelT, AcquireT) {
            self.step = WaitFutureStep::Waiting;
            (self.on_pending.take().unwrap())();
            Poll::Pending
        } else {
            Poll::Ready(false)
        }
    }

    unsafe fn poll_waiting(&mut self, cx: &mut Context) -> Poll<bool> {
        let new_waker = CopyWaker::from_waker(cx.waker().clone());
        let mut waker = self.waiter.waker().load(Acquire);
        loop {
            match waker {
                WaiterWaker::None => panic!(),
                WaiterWaker::Waiting { waker: old_waker } => {
                    if self.waiter.waker().cmpxchg_weak(
                        &mut waker, WaiterWaker::Waiting { waker: new_waker }, AcqRel, Acquire) {
                        mem::drop(old_waker.into_waker());
                        return Poll::Pending;
                    }
                }
                WaiterWaker::Done => {
                    mem::drop(new_waker.into_waker());
                    self.step = WaitFutureStep::Done;
                    return Poll::Ready(true);
                }
            }
        }
    }
}

impl<'a, A, Success, Failure, OnPending, OnCancelWoken, OnCancelSleeping>
Future for WaitFuture<'a, A, Success, Failure, OnPending, OnCancelWoken, OnCancelSleeping>
    where
        A: Packable<Raw=usize> + Debug,
        Success: IsReleaseT,
        Failure: IsLoadT,
        OnPending: FnOnce(),
        OnCancelWoken: FnOnce(),
        OnCancelSleeping: FnOnce(),
{
    type Output = bool;
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        unsafe {
            let this = self.get_unchecked_mut();
            match this.step {
                WaitFutureStep::Enter => this.poll_enter(cx),
                WaitFutureStep::Waiting => this.poll_waiting(cx),
                WaitFutureStep::Done => panic!("Polling completed future."),
            }
        }
    }
}

impl<'a, A, Success, Failure, OnPending, OnCancelWoken, OnCancelSleeping>
Drop for WaitFuture<'a, A, Success, Failure, OnPending, OnCancelWoken, OnCancelSleeping>
    where
        A: Packable<Raw=usize>,
        Success: IsReleaseT,
        Failure: IsLoadT,
        OnPending: FnOnce(),
        OnCancelWoken: FnOnce(),
        OnCancelSleeping: FnOnce(),
{
    fn drop(&mut self) {
        unsafe {
            match mem::replace(&mut self.step, WaitFutureStep::Done) {
                WaitFutureStep::Enter => {}
                WaitFutureStep::Waiting => {
                    let mut owner = self.waiter.futex().load(Relaxed);
                    loop {
                        let mut lock = (*owner).state.write().unwrap();
                        let mut owner2 = self.waiter.futex().load(Relaxed);
                        if owner == owner2 {
                            // By obtaining a write lock, we ensure that no requeue operations are
                            // in progress, so the second load is correct.
                            (*owner).flip(lock.get_mut());
                            (*owner).cancel(lock.get_mut(), self.waiter);
                            break;
                        } else {
                            // The waiter was requeued while attempting to obtain the write lock.
                            // The write lock should have blocked until that requeue operation
                            // completed, so just try again.
                            owner = owner2;
                        }
                    }
                    let mut waiter = self.waiter as *mut Waiter;
                    match waiter.waker_mut().load_mut() {
                        WaiterWaker::None => panic!(),
                        WaiterWaker::Waiting { waker } => {
                            mem::drop(waker.into_waker());
                            (self.on_cancel_sleeping.take().unwrap())();
                        }
                        WaiterWaker::Done => {
                            test_println!("Completed before canceling");
                            (self.on_cancel_woken.take().unwrap())();
                        }
                    }
                    waiter.waker_mut().store_mut(WaiterWaker::None);
                }
                WaitFutureStep::Done => {}
            }
        }
        //test_println!("Dropping {:?}", self.waiter.as_ptr());
    }
}

unsafe impl<'a, A, Success, Failure, OnPending, OnCancelWoken, OnCancelSleeping>
Send for WaitFuture<'a, A, Success, Failure, OnPending, OnCancelWoken, OnCancelSleeping>
    where
        A: Packable<Raw=usize>,
        Success: IsReleaseT,
        Failure: IsLoadT,
        OnPending: FnOnce(),
        OnCancelWoken: FnOnce(),
        OnCancelSleeping: FnOnce(),
{}
