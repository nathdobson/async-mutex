mod atomic;
mod atomic_impl;
mod state;
mod waiter;
//mod send_mutex;

use crate::futex::waiter::{FutexOwner};
use std::mem::MaybeUninit;
use crate::cell::UnsafeCell;
use std::ptr::null;
use crate::sync::Mutex;
use crate::sync::MutexGuard;
use crate::future::Future;
use std::task::{Context, Poll, Waker};
use std::pin::Pin;
use crate::sync::atomic::Ordering;
use crate::futex::state::{FutexAtom, WaiterWaker};
use crate::sync::atomic::Ordering::Acquire;
use crate::sync::atomic::Ordering::AcqRel;
use std::{mem, fmt};
use crate::sync::RwLockReadGuard;
use crate::sync::RwLock;
use crate::sync::atomic::Ordering::Relaxed;
//use crate::test_println;
use std::marker::PhantomData;
use std::fmt::{Debug, Formatter};

pub use waiter::Waiter;
pub use waiter::WaiterList;
pub use state::{CopyWaker, PANIC_WAKER_VTABLE};
#[cfg(test)]
pub use state::test_packable;
pub use atomic::{Atomic, Packable};
pub use atomic_impl::{HasAtomic, IsAtomic, usize2, usize_half, AtomicUsize2};

#[derive(Debug)]
struct FutexState {
    queues: UnsafeCell<Vec<WaiterList>>,
}

/// A synchronization primitive akin to a futex in linux.
///
/// Callers can perform atomic operations that both access a field (the "atom") and perform `wait`,
/// `notify`, or `requeue` operations. The atom
/// must be convertible to a usize (i.e. implement `Packable<Raw=usize>`). This is a
/// "multi-waiter, single-notifier" primitive; `wait` operations may occur simultaneously with any
/// operation, but `notify`/`requeue` operations must not occur simultaneously with other
/// `notify`/`requeue` operations. For a "multi-notifier single-waiter" primitive, see
/// `futures::task::AtomicWaker`.
///
/// Waiters are stored in a sequence of queues, with strict FIFO behavior within each queue. Waiters
/// select which queue they want, and the notifier may decide which queue to draw from.
///
/// Successful operations use the `AcqRel` memory ordering, but failures may use the `Acquire`
/// memory ordering.
#[derive(Debug)]
pub struct Futex {
    futex_atom: Atomic<FutexAtom>,
    state: RwLock<FutexState>,
}

#[derive(Clone, Copy)]
enum WaitFutureStep {
    Enter,
    Waiting,
    Done,
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
    pub fn new<T: Packable<Raw=usize>>(atom: T, queues: usize) -> Self {
        unsafe {
            Futex {
                futex_atom: Atomic::new(FutexAtom { atom: T::encode(atom), inbox: null() }),
                state: RwLock::new(FutexState {
                    queues: UnsafeCell::new(vec![WaiterList::new(); queues])
                }),
            }
        }
    }

    // Atomically retrieve the value
    pub unsafe fn load<A: Packable<Raw=usize>>(&self, order: Ordering) -> A {
        A::decode(self.futex_atom.load(order).atom)
    }

    // Atomically applies a function to the atom, ignoring queues.
    pub unsafe fn fetch_update<A, F>(
        &self,
        mut update: F,
    ) -> Result<A, A>
        where A: Packable<Raw=usize> + Debug,
              F: FnMut(A) -> Option<A>
    {
        let mut futex_atom = self.futex_atom.load(Acquire);
        loop {
            let atom = A::decode(futex_atom.atom);
            if let Some(atom) = update(atom) {
                let new_atom = FutexAtom { atom: A::encode(atom), inbox: futex_atom.inbox };
                if self.cmpxchg_weak::<A>(
                    &mut futex_atom, new_atom,
                    AcqRel, Acquire) {
                    return Ok(atom);
                }
            } else {
                return Err(atom);
            }
        }
    }

    /// Atomically update the atom and block if requested. Enqueue operations that <em>read-from</em>
    /// this operation will enqueue the `Waiter`.
    ///
    /// * `message`: a single usize to be included in the `Waiter`.
    /// * `queue`: the index of the queue to place the `Waiter`.
    /// * `call`: Will be called with the old atom. Returns a WaitAction specifying the new value and
    ///  whether or not to block.
    /// * `on_pending`: called after the decision to wait, but before actually waiting.
    /// * `on_cancel_woken`: called during cancellation if `Waiter::notify` has been called.
    /// * `on_cancel_sleeping`: called during cancellation if `Waiter::notify` has not been called.
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

    /// Perform a read-lock in order to access the queue. Cancellation will obtain the associated
    /// write lock. This lock is used to exclude simultaneous cancellation, not synchronize
    /// notification.
    pub fn lock(&self) -> FutexLock {
        FutexLock {
            futex: self,
            state: self.state.read().unwrap(),
        }
    }

    unsafe fn enqueue(&self, state: &FutexState, stack: *const Waiter) {
        let mut list = WaiterList::from_stack(stack);
        while let Some(waiter) = list.pop_front() {
            waiter.futex().store(FutexOwner { first: null(), second: self }, Relaxed);
            state.queues.with_mut(|queues| (*queues)[waiter.queue()].push_back(waiter));
        }
    }

    unsafe fn flip(&self, state: &mut FutexState) {
        let mut atom = self.futex_atom.load(Acquire);
        loop {
            if atom.inbox == null() {
                break;
            } else {
                let new_atom = FutexAtom { inbox: null(), ..atom };
                if self.cmpxchg_weak::<usize>(&mut atom, new_atom, AcqRel, Acquire) {
                    self.enqueue(state, atom.inbox);
                    break;
                }
            }
        }
    }

    unsafe fn cancel(&self, state: &mut FutexState, waiter: *const Waiter) {
        state.queues.with_mut(|queues| (*queues)[waiter.queue()].remove(waiter));
    }

    pub(crate) unsafe fn unsafe_debug<'a>(&'a self) -> impl 'a + Debug {
        struct Imp<'a>(&'a Futex);
        impl<'a> Debug for Imp<'a> {
            fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
                unsafe {
                    f.debug_struct("Futex")
                        .field("atom", &self.0.futex_atom)
                        .field("state", &self.0.lock().unsafe_debug()).finish()
                }
            }
        }
        Imp(self)
    }

    unsafe fn cmpxchg_weak<A: Packable<Raw=usize> + Debug>(&self, futex_atom: &mut FutexAtom, new_futex_atom: FutexAtom, success: Ordering, failure: Ordering) -> bool {
        if self.futex_atom.cmpxchg_weak(futex_atom, new_futex_atom, success, failure) {
            test_println!("from {:?}\n  to {:?}", futex_atom.debug::<A>(), new_futex_atom.debug::<A>());
            true
        } else {
            false
        }
    }
}

/// While this is a read-lock, none of the methods on a `FutexLock` or a `Waiter` may be called
/// simultaneously. Synchronization is left to the caller.
impl<'a> FutexLock<'a> {
    /// * `update`: Called with the old atom and a bool indicating if there are any `Waiter`s.
    ///
    /// Atomically updates the current atom while ensuring new `Waiter`s
    /// are placed in the appropriate queues.
    pub unsafe fn fetch_update_enqueue<A, F>(
        &self,
        mut update: F,
    ) -> (A, bool)
        where A: Packable<Raw=usize> + Debug,
              F: FnMut(A, bool) -> Option<A>
    {
        let queued = self.state.queues.with_mut(|queues| (*queues).iter().any(|queue| !queue.is_empty()));
        let mut futex_atom = self.futex.futex_atom.load(Acquire);
        loop {
            let old_atom = A::decode(futex_atom.atom);
            if futex_atom.inbox == null() {
                if let Some(atom) = update(old_atom, queued) {
                    let new_futex_atom = FutexAtom { atom: A::encode(atom), inbox: null() };
                    if self.futex.cmpxchg_weak::<A>(&mut futex_atom, new_futex_atom, AcqRel, Acquire) {
                        return (old_atom, queued);
                    }
                } else {
                    return (old_atom, queued);
                }
            } else {
                let atom;
                if let Some(decoded) = update(old_atom, true) {
                    atom = A::encode(decoded)
                } else {
                    atom = futex_atom.atom;
                }
                let new_futex_atom = FutexAtom { atom, inbox: null() };
                if self.futex.cmpxchg_weak::<A>(&mut futex_atom, new_futex_atom, AcqRel, Acquire) {
                    self.futex.enqueue(&*self.state, futex_atom.inbox);
                    return (old_atom, true);
                }
            }
        }
    }

    /// * `queue`: the index of the queue to access.
    ///
    /// Pops one `Waiter` off the queue, or returns None if there were no waiters during the last
    /// `fetch_update_enqueue` operation.
    pub unsafe fn pop(&self, queue: usize) -> Option<*const Waiter> {
        self.state.queues.with_mut(|queues| (*queues)[queue].pop_front())
    }

    /// * `count`: The maximum number of `Waiter`s to retrieve (specify `usize::MAX` for no maximum).
    /// * `queue`: The index of the queue to access.
    ///
    /// Pops up to `count` `Waiter`s off the queue, ignoring `Waiter`s added after the
    /// last `fetch_update_enqueue` operation.
    /// `enqueue` operation.
    pub unsafe fn pop_many(&self, count: usize, queue: usize) -> WaiterList {
        self.state.queues.with_mut(|queues| {
            test_println!("Queue is {:?}", (*queues));
            (*queues)[queue].pop_front_many(count)
        })
    }

    pub(crate) unsafe fn unsafe_debug<'b>(&'b self) -> impl 'b + Debug {
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

    /// Atomically transfer these waiters to another futex, as if `wait` had been called on that
    /// `Futex`.
    pub unsafe fn requeue(&self, to: &Futex, list: WaiterList) {
        if list.is_empty() {
            return;
        }
        for waiter in list {
            waiter.futex().store(FutexOwner { first: self.futex, second: to }, Relaxed);
        }
        let mut atom = to.futex_atom.load(Acquire);
        loop {
            list.head.set_prev(atom.inbox);
            let new_atom = FutexAtom { atom: atom.atom, inbox: list.tail };
            if to.cmpxchg_weak::<usize>(
                &mut atom, new_atom, AcqRel, Acquire) {
                return;
            }
        }
    }
}

impl<'a, A, R, P, Call, OnPending, OnCancelWoken, OnCancelSleeping>
WaitFuture<'a, A, R, P, Call, OnPending, OnCancelWoken, OnCancelSleeping>
    where
        A: Packable<Raw=usize> + Debug,
        Call: FnMut(A) -> WaitAction<A, R, P>,
        OnPending: for<'b> FnMut(&'b mut P),
        OnCancelWoken: FnOnce(P),
        OnCancelSleeping: FnOnce(P),
{
    unsafe fn poll_enter(&mut self, cx: &mut Context) -> Poll<Flow<R, P>> {
        let mut futex_atom = self.original_futex.futex_atom.load(Acquire);
//TODO(skip this once)
        let mut waker: Option<CopyWaker> = None;
        loop {
            let result = (self.call)(A::decode(futex_atom.atom));
            match result.flow {
                Flow::Ready(control) => {
                    if let Some(atom) = result.update {
                        let new_futex_atom = FutexAtom { atom: A::encode(atom), ..futex_atom };
                        if !self.original_futex.cmpxchg_weak::<A>(&mut futex_atom, new_futex_atom, AcqRel, Acquire) {
                            continue;
                        }
                    }
                    self.step = WaitFutureStep::Done;
                    if let Some(waker) = waker {
                        mem::drop(waker.into_waker());
                    }
                    return Poll::Ready(Flow::Ready(control));
                }
                Flow::Pending(mut control) => {
                    let mut waiter = self.waiter.as_mut_ptr();
                    (waiter as *const Waiter).set_prev(futex_atom.inbox);
                    if waker.is_none() {
                        waker = Some(CopyWaker::from_waker(cx.waker().clone()));
                        waiter.waker_mut().store_mut(WaiterWaker::Waiting { waker: waker.unwrap() });
                    }
                    waiter.futex_mut().store_mut(FutexOwner { first: null(), second: self.original_futex });
                    let mut atom =
                        if let Some(encoded) = result.update {
                            A::encode(encoded)
                        } else {
                            futex_atom.atom
                        };
                    let new_futex_atom = FutexAtom { inbox: waiter, atom };
                    if self.original_futex.cmpxchg_weak::<A>(&mut futex_atom, new_futex_atom, AcqRel, Acquire) {
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
        A: Packable<Raw=usize> + Debug,
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
