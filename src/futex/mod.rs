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
use std::{mem, fmt};
use crate::sync::RwLockReadGuard;
use crate::sync::RwLock;
use crate::sync::atomic::Ordering::Relaxed;
use crate::test_println;
use std::marker::PhantomData;
use std::fmt::{Debug, Formatter};

#[derive(Debug)]
struct FutexState {
    queues: UnsafeCell<Vec<WaiterList>>,
}

#[derive(Debug)]
pub struct Futex {
    atom: Atomic<FutexAtom>,
    state: RwLock<FutexState>,
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

pub struct FutexLock<'a> {
    futex: &'a Futex,
    state: RwLockReadGuard<'a, FutexState>,
}

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
                state: RwLock::new(FutexState {
                    queues: UnsafeCell::new(vec![WaiterList::new(); queues])
                }),
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

    pub fn lock(&self) -> FutexLock {
        FutexLock {
            futex: self,
            state: self.state.read().unwrap(),
        }
    }

    unsafe fn enqueue(&self, state: &FutexState, stack: *const Waiter) {
        let mut list = WaiterList::from_stack(stack);
        while let Some(waiter) = list.pop() {
            waiter.futex().store(FutexOwner { first: null(), second: self }, Relaxed);
            state.queues.with_mut(|queues| (*queues)[waiter.queue()].push(waiter));
        }
    }

    unsafe fn flip(&self, state: &mut FutexState) {
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
    }

    unsafe fn cancel(&self, state: &mut FutexState, waiter: *const Waiter) {
        state.queues.with_mut(|queues| (*queues)[waiter.queue()].remove(waiter));
    }

    pub unsafe fn unsafe_debug<'a>(&'a self) -> impl 'a + Debug {
        struct Imp<'a>(&'a Futex);
        impl<'a> Debug for Imp<'a> {
            fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
                unsafe {
                    f.debug_struct("Futex")
                        .field("atom", &self.0.atom)
                        .field("state", &self.0.lock().unsafe_debug()).finish()
                }
            }
        }
        Imp(self)
    }
}

impl<'a> FutexLock<'a> {
    /// Update userdata atomically. The bool indicates if the queue has waiters. Ensures the queue
    /// is flipped.
    pub unsafe fn update_flip<A, F>(
        &self,
        mut update: F,
    ) -> bool
        where A: Packable<Raw=usize> + Debug,
              F: FnMut(A, bool) -> Option<A>
    {
        let queued = self.state.queues.with_mut(|queues| (*queues).iter().any(|queue| !queue.empty()));
        let mut atom = self.futex.atom.load(Acquire);
        loop {
            if atom.inbox == null() {
                if let Some(userdata) = update(A::decode(atom.userdata), queued) {
                    let new_atom = FutexAtom { userdata: A::encode(userdata), inbox: null() };
                    if self.futex.atom.cmpxchg_weak(&mut atom, new_atom, AcqRel, Acquire) {
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
                if self.futex.atom.cmpxchg_weak(&mut atom, new_atom, AcqRel, Acquire) {
                    test_println!("{:?} -> {:?}", atom, new_atom);
                    self.futex.enqueue(&*self.state, atom.inbox);
                    return false;
                }
            }
        }
    }


    pub unsafe fn pop(&self, index: usize) -> Option<*const Waiter> {
        self.state.queues.with_mut(|queues| (*queues)[index].pop())
    }

    pub unsafe fn pop_many(&self, count: usize, queue: usize) -> WaiterList {
        self.state.queues.with_mut(|queues| {
            test_println!("Queue is {:?}", (*queues));
            (*queues)[queue].pop_many(count)
        })
    }

    pub unsafe fn unsafe_debug<'b>(&'b self) -> impl 'b + Debug {
        struct Imp<'a>(&'a FutexLock<'a>);
        impl<'a> Debug for Imp<'a> {
            fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
                unsafe {
                    self.0.state.queues.with_mut(|queues|
                        f.debug_list().entries((*queues).iter().map(|x| x as &dyn Debug)).finish()
                    )
                }
            }
        }
        Imp(self)
    }

    pub unsafe fn requeue(&self, to: &Futex, list: WaiterList) {
        if list.empty() {
            return;
        }
        for waiter in list {
            waiter.futex().store(FutexOwner { first: self.futex, second: to }, Relaxed);
        }
        let mut atom = to.atom.load(Acquire);
        loop {
            list.head.set_prev(atom.inbox);
            let new_atom = FutexAtom { userdata: atom.userdata, inbox: list.tail };
            if to.atom.cmpxchg_weak(
                &mut atom, new_atom, AcqRel, Acquire) {
                test_println!("{:?} -> {:?}", atom, new_atom);
                return;
            }
        }
    }

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
                        (*owner.second).flip(&mut *second);
                        if owner.first == null() {
                            let mut owner2 = self.waiter.as_ptr().futex().load(Relaxed);
                            if owner2 == owner {
                                (*owner.second).cancel(&mut *second, self.waiter.as_ptr());
                                break;
                            }
                        } else {
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
