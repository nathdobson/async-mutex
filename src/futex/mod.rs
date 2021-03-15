pub mod atomic;
pub mod atomic_impl;
pub mod state;
pub mod waiter;

use crate::futex::atomic::{Atomic, Packable};
use crate::futex::waiter::{Waiter, WaiterList, FutexOwner};
use std::mem::MaybeUninit;
use crate::cell::UnsafeCell;
use std::ptr::null;
use crate::sync::Mutex;
use crate::sync::MutexGuard;
use crate::future::Future;
use std::task::{Context, Poll, Waker};
use std::pin::Pin;
use crate::sync::atomic::Ordering;
use crate::futex::state::{FutexAtom, CopyWaker, WaiterWaker};
use crate::sync::atomic::Ordering::Acquire;
use crate::sync::atomic::Ordering::AcqRel;
use std::mem;
use crate::sync::RwLockReadGuard;
use crate::sync::RwLock;
use crate::sync::atomic::Ordering::Relaxed;
use crate::test_println;
use std::marker::PhantomData;
use std::fmt::Debug;

#[derive(Debug)]
pub struct FutexState<M> {
    queue: WaiterList<M>
}

#[derive(Debug)]
pub struct Futex<M> {
    atom: Atomic<FutexAtom<M>>,
    state: RwLock<UnsafeCell<FutexState<M>>>,
}

#[derive(Clone, Copy)]
enum WaitFutureStep {
    Enter,
    Waiting,
    Done,
}

pub struct RequeueResult<M, A> {
    pub(crate) userdata: Option<A>,
    pub(crate) list: WaiterList<M>,
}

pub enum Flow<R, P> {
    Ready(R),
    Pending(P),
}

pub struct WaitAction<A, R, P> {
    pub update: Option<A>,
    pub flow: Flow<R, P>,
}

// pub struct WaitImpl<Call, OnPending, OnCancelWoken, OnCancelSleeping> {
//     pub call: Call,
//     pub on_pending: OnPending,
//     pub on_cancel_woken: OnCancelWoken,
//     pub on_cancel_sleeping: OnCancelSleeping,
// }
//
// pub trait Wait<M, A, R, P> where A: Packable<Raw=usize> {
//     fn call(&mut self, userdata: A) -> WaitAction<A, R, P>;
//     fn on_pending(&mut self, msg: &M, pending: &mut P);
//     fn on_cancel_woken(self, msg: M, pending: P);
//     fn on_cancel_sleeping(self, msg: M, pending: P);
// }
//
// pub fn wait_impl<M, A, R, P, Call, OnPending, OnCancelWoken, OnCancelSleeping>(
//     call: Call,
//     on_pending: OnPending,
//     on_cancel_woken: OnCancelWoken,
//     on_cancel_sleeping: OnCancelSleeping,
// ) -> impl Wait<M, A, R, P>
//     where
//         A: Packable<Raw=usize>,
//         Call: FnMut(A) -> WaitAction<A, R, P>,
//         OnPending: for<'a> FnMut(&'a M, &'a mut P),
//         OnCancelWoken: FnOnce(M, P),
//         OnCancelSleeping: FnOnce(M, P),
// {
//     WaitImpl { call, on_pending, on_cancel_woken, on_cancel_sleeping }
// }
//
// impl<M, A, R, P, Call, OnPending, OnCancelWoken, OnCancelSleeping>
// Wait<M, A, R, P> for
// WaitImpl<Call, OnPending, OnCancelWoken, OnCancelSleeping> where
//     A: Packable<Raw=usize>,
//     Call: FnMut(A) -> WaitAction<A, R, P>,
//     OnPending: for<'a> FnMut(&'a M, &'a mut P),
//     OnCancelWoken: FnOnce(M, P),
//     OnCancelSleeping: FnOnce(M, P),
// {
//     fn call(&mut self, userdata: A) -> WaitAction<A, R, P> {
//         (self.call)(userdata)
//     }
//     fn on_pending(&mut self, msg: &M, pending: &mut P) {
//         (self.on_pending)(msg, pending)
//     }
//     fn on_cancel_woken(self, msg: M, pending: P) {
//         (self.on_cancel_woken)(msg, pending)
//     }
//     fn on_cancel_sleeping(self, msg: M, pending: P) {
//         (self.on_cancel_sleeping)(msg, pending)
//     }
// }

#[must_use = "with does nothing unless polled/`await`-ed"]
pub struct WaitFuture<'a, M, A, R, P, Call, OnPending, OnCancelWoken, OnCancelSleeping>
    where
        A: Packable<Raw=usize>,
        Call: FnMut(A) -> WaitAction<A, R, P>,
        OnPending: for<'b> FnMut(&'b UnsafeCell<M>, &'b mut P),
        OnCancelWoken: FnOnce(M, P),
        OnCancelSleeping: FnOnce(M, P),
{
    original_futex: &'a Futex<M>,
    step: WaitFutureStep,
    // TODO MaybeUninit isn't exactly right here: the wrapper should communicate to
    // the compiler that it is unsafe to convert a `&mut LockFuture` to a `&mut Waiter`,
    // while it is safe to convert an `&mut LockFuture` to a `&Waiter`.
    waiter: MaybeUninit<Waiter<M>>,
    call: Call,
    on_pending: OnPending,
    on_cancel_woken: Option<OnCancelWoken>,
    on_cancel_sleeping: Option<OnCancelSleeping>,
    result: Option<P>,
    phantom: PhantomData<(A, R)>,
}

impl<M> Futex<M> {
    pub fn new<T: Packable<Raw=usize>>(userdata: T) -> Self {
        unsafe {
            Futex {
                atom: Atomic::new(FutexAtom { userdata: T::encode(userdata), inbox: null() }),
                state: RwLock::new(UnsafeCell::new(FutexState {
                    queue: WaiterList::new()
                })),
            }
        }
    }
    pub unsafe fn load<A: Packable<Raw=usize>>(&self, order: Ordering) -> A {
        A::decode(self.atom.load(order).userdata)
    }

    pub unsafe fn wait<A, R, P, Call, OnPending, OnCancelWoken, OnCancelSleeping>(
        &self,
        message: M,
        call: Call,
        on_pending: OnPending,
        on_cancel_woken: OnCancelWoken,
        on_cancel_sleeping: OnCancelSleeping,
    ) -> WaitFuture<M, A, R, P, Call, OnPending, OnCancelWoken, OnCancelSleeping> where
        A: Packable<Raw=usize>,
        Call: FnMut(A) -> WaitAction<A, R, P>,
        OnPending: for<'a> FnMut(&'a UnsafeCell<M>, &'a mut P),
        OnCancelWoken: FnOnce(M, P),
        OnCancelSleeping: FnOnce(M, P),
    {
        WaitFuture {
            original_futex: self,
            step: WaitFutureStep::Enter,
            waiter: MaybeUninit::new(Waiter::new(message)),
            call,
            on_pending,
            on_cancel_woken: Some(on_cancel_woken),
            on_cancel_sleeping: Some(on_cancel_sleeping),
            result: None,
            phantom: PhantomData,
        }
    }
    // pub unsafe fn wait_impl<A, R, P, W>(
    //     &self,
    //     message: M,
    //     wait: W,
    // ) -> WaitFuture<M, A, R, P, W> where
    //     A: Packable<Raw=usize>,
    //     W: Wait<M, A, R, P>
    // {
    //     WaitFuture {
    //         original_futex: self,
    //         step: WaitFutureStep::Entering,
    //         waiter: MaybeUninit::new(Waiter::new(message)),
    //         wait: Some(wait),
    //         result: None,
    //         phantom: PhantomData,
    //     }
    // }
    pub unsafe fn lock_state(&self) -> RwLockReadGuard<UnsafeCell<FutexState<M>>> {
        self.state.read().unwrap()
    }

    /// Update userdata atomically. The bool indicates if the queue has waiters. Ensures the queue
    /// is flipped.
    pub unsafe fn update_flip<A, F>(
        &self,
        state: &mut FutexState<M>,
        mut update: F,
    ) -> bool
        where A: Packable<Raw=usize> + Debug,
              F: FnMut(A, bool) -> Option<A>
    {
        let mut atom = self.atom.load(Acquire);
        loop {
            if atom.inbox == null() {
                if let Some(userdata) = update(A::decode(atom.userdata), !state.queue.empty()) {
                    let new_atom = FutexAtom { userdata: A::encode(userdata), inbox: null() };
                    if self.atom.cmpxchg_weak(&mut atom, new_atom, AcqRel, Acquire) {
                        test_println!("{:?} -> {:?}", atom, new_atom);
                        return true;
                    }
                } else {
                    return true;
                }
            } else {
                let userdata;
                if let Some(decoded) = update(A::decode(atom.userdata), true) {
                    userdata = A::encode(decoded)
                } else {
                    userdata = atom.userdata;
                }
                let new_atom = FutexAtom { userdata, inbox: null() };
                if self.atom.cmpxchg_weak(&mut atom, new_atom, AcqRel, Acquire) {
                    test_println!("{:?} -> {:?}", atom, new_atom);
                    let list = WaiterList::from_stack(atom.inbox);
                    for waiter in list {
                        waiter.futex().store(FutexOwner { first: null(), second: self }, Relaxed);
                    }
                    state.queue.append(list);
                    atom.inbox = null();
                    return false;
                }
            }
        }
    }

    pub unsafe fn pop(&self, state: &mut FutexState<M>) -> Option<*const Waiter<M>> {
        state.queue.pop()
    }

    pub unsafe fn pop_many(&self, state: &mut FutexState<M>, count: usize) -> WaiterList<M> {
        test_println!("Queue is {:?}", state.queue);
        state.queue.pop_many(count)
    }

    unsafe fn cancel(&self, state: &mut FutexState<M>, waiter: *const Waiter<M>) {
        test_println!("Canceling {:?}", waiter);
        let mut atom = self.atom.load(Acquire);
        loop {
            if atom.inbox == null() {
                break;
            } else {
                let new_atom = FutexAtom { inbox: null(), ..atom };
                if self.atom.cmpxchg_weak(&mut atom, new_atom, AcqRel, Acquire) {
                    test_println!("{:?} -> {:?}", atom, new_atom);
                    state.queue.append_stack(atom.inbox);
                    atom.inbox = null();
                    break;
                }
            }
        }
        state.queue.remove(waiter);
    }

    pub unsafe fn requeue(&self, from: &Futex<M>, list: WaiterList<M>) {
        if list.empty() {
            return;
        }
        for waiter in list {
            waiter.futex().store(FutexOwner { first: from, second: self }, Relaxed);
        }
        let mut atom = self.atom.load(Acquire);
        loop {
            list.head.set_prev(atom.inbox);
            let new_atom = FutexAtom { userdata: atom.userdata, inbox: list.tail };
            if self.atom.cmpxchg_weak(
                &mut atom, new_atom, AcqRel, Acquire) {
                test_println!("{:?} -> {:?}", atom, new_atom);
                return;
            }
        }
    }

    // pub unsafe fn requeue_update<A, F>(&self, update: F) where
    //     A: Packable<Raw=usize>,
    //     F: FnMut(A) -> RequeueResult<A> {
    //     let mut atom = self.atom.load(Acquire);
    //     loop {
    //         let result = update(A::decode(atom.userdata));
    //         let mut new_atom = atom;
    //         let mut old_prev = null();
    //         if result.list.empty() {
    //             if result.userdata.is_none() {
    //                 return;
    //             }
    //         } else {
    //             old_prev = result.list.head.prev();
    //             result.list.head.set_prev(atom.inbox);
    //             new_atom.inbox = result.list.tail;
    //         }
    //         if let Some(userdata) = result.userdata {
    //             new_atom.userdata = A::encode(userdata);
    //         }
    //         if self.atom.cmpxchg_weak(&mut atom, new_atom, AcqRel, AcqRel) {
    //             return;
    //         } else {
    //             if !result.list.empty() {
    //                 result.list.head.set_prev(old_prev);
    //             }
    //         }
    //     }
    // }
}

impl<'a, M, A, R, P, Call, OnPending, OnCancelWoken, OnCancelSleeping>
WaitFuture<'a, M, A, R, P, Call, OnPending, OnCancelWoken, OnCancelSleeping>
    where
        A: Packable<Raw=usize>,
        Call: FnMut(A) -> WaitAction<A, R, P>,
        OnPending: for<'b> FnMut(&'b UnsafeCell<M>, &'b mut P),
        OnCancelWoken: FnOnce(M, P),
        OnCancelSleeping: FnOnce(M, P),
{
    unsafe fn poll_enter(&mut self, cx: &mut Context) -> Poll<(Flow<R, P>, M)> {
        let mut atom = self.original_futex.atom.load(Acquire);
//TODO(skip this once)
        let waker = CopyWaker::from_waker(cx.waker().clone());
        loop {
            let result = (self.call)(A::decode(atom.userdata));
            match result.flow {
                Flow::Ready(control) => {
                    if let Some(userdata) = result.update {
                        let new_atom = FutexAtom { userdata: A::encode(userdata), ..atom };
                        if self.original_futex.atom.cmpxchg_weak(&mut atom, new_atom, AcqRel, Acquire) {
                            test_println!("{:?} -> {:?}", atom, new_atom);
                            mem::drop(waker.into_waker());
                            self.step = WaitFutureStep::Done;
                        } else {
                            continue;
                        }
                    } else {
                        mem::drop(waker.into_waker());
                    }
                    return Poll::Ready((Flow::Ready(control), self.waiter.assume_init_mut().take_message()));
                }
                Flow::Pending(mut control) => {
                    let mut waiter = self.waiter.as_mut_ptr();
                    (waiter as *const Waiter<M>).set_prev(atom.inbox);
                    waiter.waker_mut().store_mut(WaiterWaker::Waiting { waker });
                    waiter.futex_mut().store_mut(FutexOwner { first: null(), second: self.original_futex });
                    let mut userdata =
                        if let Some(encoded) = result.update {
                            A::encode(encoded)
                        } else {
                            atom.userdata
                        };
                    let new_atom = FutexAtom { inbox: waiter, userdata };
                    if self.original_futex.atom.cmpxchg_weak(&mut atom, new_atom, AcqRel, Acquire) {
                        test_println!("{:?} -> {:?}", atom, new_atom);
                        self.step = WaitFutureStep::Waiting;
                        (self.on_pending)(self.waiter.as_ptr().message(), &mut control);
                        self.result = Some(control);
                        return Poll::Pending;
                    }
                }
            }
        }
    }

    unsafe fn poll_waiting(&mut self, cx: &mut Context) -> Poll<(Flow<R, P>, M)> {
        let new_waker = CopyWaker::from_waker(cx.waker().clone());
        let mut waker = self.waiter.as_ptr().waker().load(Acquire);
        loop {
            match waker {
                WaiterWaker::None => panic!(),
                WaiterWaker::Waiting { waker: old_waker } => {
                    if self.waiter.as_ptr().waker().cmpxchg_weak(
                        &mut waker, WaiterWaker::Waiting { waker: new_waker }, AcqRel, Acquire) {
                        mem::drop(old_waker.into_waker());
                        return Poll::Pending;
                    }
                }
                WaiterWaker::Done => {
                    mem::drop(new_waker.into_waker());
                    self.step = WaitFutureStep::Done;
                    return Poll::Ready((Flow::Pending(self.result.take().unwrap()),
                                        self.waiter.assume_init_mut().take_message()),
                    );
                }
            }
        }
    }
}

impl<'a, M, A, R, P, Call, OnPending, OnCancelWoken, OnCancelSleeping>
Future for WaitFuture<'a, M, A, R, P, Call, OnPending, OnCancelWoken, OnCancelSleeping>
    where
        A: Packable<Raw=usize>,
        Call: FnMut(A) -> WaitAction<A, R, P>,
        OnPending: for<'b> FnMut(&'b UnsafeCell<M>, &'b mut P),
        OnCancelWoken: FnOnce(M, P),
        OnCancelSleeping: FnOnce(M, P),
{
    type Output = (Flow<R, P>, M);
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

impl<'a, M, A, R, P, Call, OnPending, OnCancelWoken, OnCancelSleeping>
Drop for WaitFuture<'a, M, A, R, P, Call, OnPending, OnCancelWoken, OnCancelSleeping>
    where
        A: Packable<Raw=usize>,
        Call: FnMut(A) -> WaitAction<A, R, P>,
        OnPending: for<'b> FnMut(&'b UnsafeCell<M>, &'b mut P),
        OnCancelWoken: FnOnce(M, P),
        OnCancelSleeping: FnOnce(M, P),
{
    fn drop(&mut self) {
        unsafe {
            match mem::replace(&mut self.step, WaitFutureStep::Done) {
                WaitFutureStep::Enter => {}
                WaitFutureStep::Waiting => {
                    loop {
                        let mut owner = self.waiter.as_ptr().futex().load(Relaxed);
                        let mut second = (*owner.second).state.write().unwrap();
                        if owner.first == null() {
                            let mut owner2 = self.waiter.as_ptr().futex().load(Relaxed);
                            if owner2 == owner {
                                second.with_mut(|second| {
                                    (*owner.second).cancel(&mut *second, self.waiter.as_ptr());
                                });
                                break;
                            }
                        } else {
                            if second.with_mut(|second| {
                                (*owner.second).update_flip(&mut *second, |_: usize, _| None);
                                let mut owner2 = self.waiter.as_ptr().futex().load(Relaxed);
                                if owner2 == (FutexOwner { first: null(), second: owner.second }) {
                                    (*owner.second).cancel(&mut *second, self.waiter.as_ptr());
                                    return true;
                                }
                                false
                            }) {
                                break;
                            }
                            mem::drop(second);
                            mem::drop((*owner.first).state.write().unwrap());
                        }
                    }
                    match self.waiter.as_mut_ptr().waker_mut().load_mut() {
                        WaiterWaker::None => panic!(),
                        WaiterWaker::Waiting { waker } => {
                            mem::drop(waker.into_waker());
                            (self.on_cancel_sleeping.take().unwrap())(self.waiter.assume_init_mut().take_message(), self.result.take().unwrap());
                        }
                        WaiterWaker::Done => {
                            test_println!("Completed before canceling");
                            (self.on_cancel_woken.take().unwrap())(self.waiter.assume_init_mut().take_message(), self.result.take().unwrap());
                        }
                    }
                }
                WaitFutureStep::Done => {}
            }
        }
        //test_println!("Dropping {:?}", self.waiter.as_ptr());
    }
}

unsafe impl<'a, M, A, R, P, Call, OnPending, OnCancelWoken, OnCancelSleeping>
Send for WaitFuture<'a, M, A, R, P, Call, OnPending, OnCancelWoken, OnCancelSleeping>
    where
        A: Packable<Raw=usize>,
        Call: FnMut(A) -> WaitAction<A, R, P>,
        OnPending: for<'b> FnMut(&'b UnsafeCell<M>, &'b mut P),
        OnCancelWoken: FnOnce(M, P),
        OnCancelSleeping: FnOnce(M, P),
{}

unsafe impl<M> Send for Futex<M> {}

unsafe impl<M> Sync for Futex<M> {}
