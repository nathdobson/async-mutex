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
pub struct FutexState {
    queues: Vec<WaiterList>,
}

#[derive(Debug)]
pub struct Futex {
    atom: Atomic<FutexAtom>,
    state: RwLock<UnsafeCell<FutexState>>,
}

#[derive(Clone, Copy)]
enum WaitFutureStep {
    Enter,
    Waiting,
    Done,
}

pub struct RequeueResult<A> {
    pub(crate) userdata: Option<A>,
    pub(crate) list: WaiterList,
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
pub struct WaitFuture<'a, A, R, P, Call, OnPending, OnCancelWoken, OnCancelSleeping>
    where
        A: Packable<Raw=usize>,
        Call: FnMut(A) -> WaitAction<A, R, P>,
        OnPending: for<'b> FnMut(&'b mut P),
        OnCancelWoken: FnOnce(P),
        OnCancelSleeping: FnOnce(P),
{
    original_futex: &'a Futex,
    step: WaitFutureStep,
    // TODO MaybeUninit isn't exactly right here: the wrapper should communicate to
    // the compiler that it is unsafe to convert a `&mut LockFuture` to a `&mut Waiter`,
    // while it is safe to convert an `&mut LockFuture` to a `&Waiter`.
    waiter: MaybeUninit<Waiter>,
    call: Call,
    on_pending: OnPending,
    on_cancel_woken: Option<OnCancelWoken>,
    on_cancel_sleeping: Option<OnCancelSleeping>,
    result: Option<P>,
    phantom: PhantomData<(A, R)>,
}

impl Futex {
    pub fn new<T: Packable<Raw=usize>>(userdata: T, queues: usize) -> Self {
        unsafe {
            Futex {
                atom: Atomic::new(FutexAtom { userdata: T::encode(userdata), inbox: null() }),
                state: RwLock::new(UnsafeCell::new(FutexState {
                    queues: vec![WaiterList::new(); queues]
                })),
            }
        }
    }

    pub unsafe fn load<A: Packable<Raw=usize>>(&self, order: Ordering) -> A {
        A::decode(self.atom.load(order).userdata)
    }

    pub unsafe fn update<A, F>(
        &self,
        mut update: F,
    ) -> Result<A, A>
        where A: Packable<Raw=usize> + Debug,
              F: FnMut(A) -> Option<A>
    {
        let mut atom = self.atom.load(Acquire);
        loop {
            let userdata = A::decode(atom.userdata);
            if let Some(userdata) = update(userdata) {
                let new_atom = FutexAtom { userdata: A::encode(userdata), inbox: atom.inbox };
                if self.atom.cmpxchg_weak(
                    &mut atom, new_atom,
                    AcqRel, Acquire) {
                    return Ok(userdata);
                }
            } else {
                return Err(userdata);
            }
        }
    }

    pub unsafe fn wait<A, R, P, Call, OnPending, OnCancelWoken, OnCancelSleeping>(
        &self,
        message: usize,
        queue: usize,
        call: Call,
        on_pending: OnPending,
        on_cancel_woken: OnCancelWoken,
        on_cancel_sleeping: OnCancelSleeping,
    ) -> WaitFuture<A, R, P, Call, OnPending, OnCancelWoken, OnCancelSleeping> where
        A: Packable<Raw=usize>,
        Call: FnMut(A) -> WaitAction<A, R, P>,
        OnPending: for<'a> FnMut(&'a mut P),
        OnCancelWoken: FnOnce(P),
        OnCancelSleeping: FnOnce(P),
    {
        WaitFuture {
            original_futex: self,
            step: WaitFutureStep::Enter,
            waiter: MaybeUninit::new(Waiter::new(message, queue)),
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
    pub unsafe fn lock_state(&self) -> RwLockReadGuard<UnsafeCell<FutexState>> {
        self.state.read().unwrap()
    }

    /// Update userdata atomically. The bool indicates if the queue has waiters. Ensures the queue
    /// is flipped.
    pub unsafe fn update_flip<A, F>(
        &self,
        state: &UnsafeCell<FutexState>,
        mut update: F,
    ) -> bool
        where A: Packable<Raw=usize> + Debug,
              F: FnMut(A, bool) -> Option<A>
    {
        let queued = state.with_mut(|state| (*state).queues.iter().any(|queue| !queue.empty()));
        let mut atom = self.atom.load(Acquire);
        loop {
            if atom.inbox == null() {
                if let Some(userdata) = update(A::decode(atom.userdata), queued) {
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
                    self.enqueue(state, atom.inbox);
                    return false;
                }
            }
        }
    }

    unsafe fn enqueue(&self, state: &UnsafeCell<FutexState>, stack: *const Waiter) {
        let mut list = WaiterList::from_stack(stack);
        while let Some(waiter) = list.pop() {
            waiter.futex().store(FutexOwner { first: null(), second: self }, Relaxed);
            state.with_mut(|state| (*state).queues[waiter.queue()].push(waiter));
        }
    }

    pub unsafe fn pop(&self, state: &UnsafeCell<FutexState>, index: usize) -> Option<*const Waiter> {
        state.with_mut(|state| (*state).queues[index].pop())
    }

    pub unsafe fn pop_many(&self, state: &UnsafeCell<FutexState>, count: usize, queue: usize) -> WaiterList {
        state.with_mut(|state| {
            test_println!("Queue is {:?}", (*state).queues);
            (*state).queues[queue].pop_many(count)
        })
    }

    unsafe fn cancel(&self, state: &mut UnsafeCell<FutexState>, waiter: *const Waiter) {
        test_println!("Canceling {:?}", waiter);
        let mut atom = self.atom.load(Acquire);
        loop {
            if atom.inbox == null() {
                break;
            } else {
                let new_atom = FutexAtom { inbox: null(), ..atom };
                if self.atom.cmpxchg_weak(&mut atom, new_atom, AcqRel, Acquire) {
                    test_println!("{:?} -> {:?}", atom, new_atom);
                    self.enqueue(state, atom.inbox);
                    break;
                }
            }
        }
        state.with_mut(|state| (*state).queues[waiter.queue()].remove(waiter));
    }

    pub unsafe fn requeue(&self, from: &Futex, list: WaiterList) {
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

impl<'a, A, R, P, Call, OnPending, OnCancelWoken, OnCancelSleeping>
WaitFuture<'a, A, R, P, Call, OnPending, OnCancelWoken, OnCancelSleeping>
    where
        A: Packable<Raw=usize>,
        Call: FnMut(A) -> WaitAction<A, R, P>,
        OnPending: for<'b> FnMut(&'b mut P),
        OnCancelWoken: FnOnce(P),
        OnCancelSleeping: FnOnce(P),
{
    unsafe fn poll_enter(&mut self, cx: &mut Context) -> Poll<Flow<R, P>> {
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
                    return Poll::Ready(Flow::Ready(control));
                }
                Flow::Pending(mut control) => {
                    let mut waiter = self.waiter.as_mut_ptr();
                    (waiter as *const Waiter).set_prev(atom.inbox);
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
                        (self.on_pending)(&mut control);
                        self.result = Some(control);
                        return Poll::Pending;
                    }
                }
            }
        }
    }

    unsafe fn poll_waiting(&mut self, cx: &mut Context) -> Poll<Flow<R, P>> {
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
                    return Poll::Ready(Flow::Pending(self.result.take().unwrap()),
                    );
                }
            }
        }
    }
}

impl<'a, A, R, P, Call, OnPending, OnCancelWoken, OnCancelSleeping>
Future for WaitFuture<'a, A, R, P, Call, OnPending, OnCancelWoken, OnCancelSleeping>
    where
        A: Packable<Raw=usize>,
        Call: FnMut(A) -> WaitAction<A, R, P>,
        OnPending: for<'b> FnMut(&'b mut P),
        OnCancelWoken: FnOnce(P),
        OnCancelSleeping: FnOnce(P),
{
    type Output = Flow<R, P>;
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

impl<'a, A, R, P, Call, OnPending, OnCancelWoken, OnCancelSleeping>
Drop for WaitFuture<'a, A, R, P, Call, OnPending, OnCancelWoken, OnCancelSleeping>
    where
        A: Packable<Raw=usize>,
        Call: FnMut(A) -> WaitAction<A, R, P>,
        OnPending: for<'b> FnMut(&'b mut P),
        OnCancelWoken: FnOnce(P),
        OnCancelSleeping: FnOnce(P),
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
                                (*owner.second).cancel(&mut *second, self.waiter.as_ptr());
                                break;
                            }
                        } else {
                            (*owner.second).update_flip(&mut *second, |_: usize, _| None);
                            let mut owner2 = self.waiter.as_ptr().futex().load(Relaxed);
                            if owner2 == (FutexOwner { first: null(), second: owner.second }) {
                                (*owner.second).cancel(&mut *second, self.waiter.as_ptr());
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
                            (self.on_cancel_sleeping.take().unwrap())(self.result.take().unwrap());
                        }
                        WaiterWaker::Done => {
                            test_println!("Completed before canceling");
                            (self.on_cancel_woken.take().unwrap())(self.result.take().unwrap());
                        }
                    }
                }
                WaitFutureStep::Done => {}
            }
        }
        //test_println!("Dropping {:?}", self.waiter.as_ptr());
    }
}

unsafe impl<'a, A, R, P, Call, OnPending, OnCancelWoken, OnCancelSleeping>
Send for WaitFuture<'a, A, R, P, Call, OnPending, OnCancelWoken, OnCancelSleeping>
    where
        A: Packable<Raw=usize>,
        Call: FnMut(A) -> WaitAction<A, R, P>,
        OnPending: for<'b> FnMut(&'b mut P),
        OnCancelWoken: FnOnce(P),
        OnCancelSleeping: FnOnce(P),
{}

unsafe impl Send for Futex {}

unsafe impl Sync for Futex {}
