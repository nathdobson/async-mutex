use std::{fmt, mem};
use std::fmt::{Debug, Formatter};
//use crate::test_println;
use std::marker::PhantomData;
use std::mem::MaybeUninit;
use std::ops::DerefMut;
use std::pin::Pin;
use std::ptr::null;
use std::task::{Context, Poll, Waker};

#[cfg(test)]
pub use state::test_packable;
pub use waiter::Waiter;
pub use state::FutexAtom;
pub use waiter::WaiterHandle;
pub use waiter::WaiterHandleList;
pub use waiter::WaiterHandleIntoIter;
pub use waiter::WaiterHandleIterMut;
pub use crate::futex::wait_future::WaitFuture;

use crate::atomic::{Atomic, CopyWaker, Packable, IsOrderingT, AcqRelT, AcquireT, IsAcquireT, IsReleaseT, IsLoadT};
use crate::atomic::{AtomicUsize2, HasAtomic, IsAtomic, usize2, usize_half};
use crate::cell::UnsafeCell;
use crate::futex::atomic_refcell::{AtomicRefCell, RefMut};
use crate::futex::state::{RawFutexAtom, WaiterWaker};
use crate::futex::waiter::{FutexOwner, WaiterList};
use crate::future::Future;
use crate::sync::atomic::Ordering;
use crate::sync::atomic::Ordering::AcqRel;
use crate::sync::atomic::Ordering::Acquire;
use crate::sync::atomic::Ordering::Relaxed;
use crate::sync::Mutex;
use crate::sync::MutexGuard;
use crate::sync::RwLock;
use crate::sync::RwLockReadGuard;
use crate::futex::wait_future::WaitFutureStep;


mod state;
mod waiter;
mod atomic_refcell;
mod wait_future;

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
pub struct Futex<A: Packable<Raw=usize>> {
    raw: RawFutex,
    phantom: PhantomData<A>,
}

#[derive(Debug)]
struct RawFutex {
    futex_atom: Atomic<RawFutexAtom>,
    state: RwLock<AtomicRefCell<FutexState>>,
}

#[derive(Debug)]
struct FutexState {
    queues: Vec<WaiterList>,
}

pub struct FutexWaitersGuard<'futex, A: Packable<Raw=usize>> {
    futex: &'futex Futex<A>,
    read_guard: RwLockReadGuard<'futex, AtomicRefCell<FutexState>>,
}

pub struct FutexQueueGuard<'waiters, 'futex, A: Packable<Raw=usize>> {
    futex: &'futex Futex<A>,
    ref_mut: RefMut<'waiters, FutexState>,
}

fn assert_variance1<'a: 'b, 'b, A: Packable<Raw=usize>>(x: FutexWaitersGuard<'a, A>) -> FutexWaitersGuard<'b, A> { x }

fn assert_variance2<'a1: 'b1, 'b1, 'a2: 'b2, 'b2, A: Packable<Raw=usize>>(x: FutexQueueGuard<'a1, 'a2, A>) -> FutexQueueGuard<'b1, 'b2, A> { x }

impl<A: Packable<Raw=usize>> Futex<A> {
    pub fn new<T: Packable<Raw=usize>>(atom: T, queues: usize) -> Self {
        unsafe {
            Futex {
                raw: RawFutex {
                    futex_atom: Atomic::new(RawFutexAtom { atom: T::encode(atom), inbox: null() }),
                    state: RwLock::new(AtomicRefCell::new(FutexState {
                        queues: vec![WaiterList::new(); queues]
                    })),
                },
                phantom: PhantomData,
            }
        }
    }

    // Atomically retrieve the state
    pub fn load(&self, order: Ordering) -> FutexAtom<A> {
        FutexAtom { raw: self.raw.futex_atom.load(order), phantom: PhantomData }
    }

    #[must_use = "Failure might be spurious, which should be handled."]
    pub fn cmpxchg_weak(&self, current: &mut FutexAtom<A>, new: A, success: impl IsOrderingT, failure: impl IsLoadT) -> bool {
        unsafe {
            let new = RawFutexAtom { atom: A::encode(new), inbox: current.raw.inbox };
            self.raw.cmpxchg_weak::<A, _, _>(&mut current.raw, new, success, failure)
        }
    }

    pub fn waiter(&self, message: usize, queue: usize) -> Waiter {
        Waiter::new(self, message, queue)
    }

    #[must_use = "Failure might be spurious, which should be handled."]
    pub async fn cmpxchg_wait_weak<'a>(
        &'a self,
        mut waiter: Pin<&'a mut Waiter>,
        current: &'a mut FutexAtom<A>,
        new: A,
        success: impl IsReleaseT,
        failure: impl IsLoadT,
        on_pending: impl 'a + FnOnce(),
        on_cancel_woken: impl 'a + FnOnce(),
        on_cancel_sleeping: impl 'a + FnOnce(),
    ) -> bool where {
        self.cmpxchg_wait_weak_impl(waiter, current, new, success, failure, on_pending, on_cancel_woken, on_cancel_sleeping).await
    }

    #[must_use = "Failure might be spurious, which should be handled."]
    pub fn cmpxchg_wait_weak_impl<'a, Success, Failure, OnPending, OnCancelWoken, OnCancelSleeping>(
        &'a self,
        mut waiter: Pin<&'a mut Waiter>,
        current: &'a mut FutexAtom<A>,
        new: A,
        success: Success,
        failure: Failure,
        on_pending: OnPending,
        on_cancel_woken: OnCancelWoken,
        on_cancel_sleeping: OnCancelSleeping,
    ) -> WaitFuture<'a, A, Success, Failure, OnPending, OnCancelWoken, OnCancelSleeping> where
        A: Packable<Raw=usize>,
        Success: IsReleaseT,
        Failure: IsLoadT,
        OnPending: FnOnce(),
        OnCancelWoken: FnOnce(),
        OnCancelSleeping: FnOnce(),
    {
        unsafe {
            assert!((waiter.as_mut().get_unchecked_mut() as *mut Waiter).futex_mut().load_mut() == FutexOwner { first: null(), second: &self.raw });
        }
        WaitFuture {
            original_futex: self,
            current,
            new,
            step: WaitFutureStep::Enter,
            waiter: &*waiter,
            on_pending: Some(on_pending),
            on_cancel_woken: Some(on_cancel_woken),
            on_cancel_sleeping: Some(on_cancel_sleeping),
            phantom: PhantomData,
        }
    }

    /// Perform a read-lock in order to access the queue. Cancellation will obtain the associated
    /// write lock. This lock is used to exclude simultaneous cancellation, not synchronize
    /// notification.
    pub fn lock<'futex>(&'futex self) -> FutexWaitersGuard<'futex, A> {
        FutexWaitersGuard {
            futex: self,
            read_guard: self.raw.state.read().unwrap(),
        }
    }

    /// Atomically transfer these waiters to another futex, as if `wait` had been called on that
    /// `Futex`.
    pub fn requeue<'futex>(&self, mut list: WaiterHandleList<'futex>) {
        unsafe {
            if list.is_empty() {
                return;
            }
            for waiter in &mut list {
                let old = (waiter as *mut Waiter).futex_mut().load_mut().second;
                (waiter as *mut Waiter).futex_mut().store_mut(FutexOwner { first: old, second: &self.raw });
            }
            let mut atom = self.raw.futex_atom.load(Acquire);
            let list = list.raw;
            loop {
                list.head.set_prev(atom.inbox);
                let new_atom = RawFutexAtom { atom: atom.atom, inbox: list.tail };
                if self.raw.cmpxchg_weak::<usize, _, _>(
                    &mut atom, new_atom, AcqRelT, AcquireT) {
                    return;
                }
            }
        }
    }

    pub(crate) unsafe fn unsafe_debug<'a>(&'a self) -> impl 'a + Debug {
        struct Imp<'a, A: Packable<Raw=usize>>(&'a Futex<A>);
        impl<'a, A: Packable<Raw=usize>> Debug for Imp<'a, A> {
            fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
                unsafe {
                    f.debug_struct("Futex")
                        .field("atom", &self.0.raw.futex_atom)
                        .field("state", &self.0.lock().lock().unsafe_debug()).finish()
                }
            }
        }
        Imp(self)
    }
}

impl RawFutex {
    unsafe fn flip(&self, state: &mut FutexState) {
        let mut atom = self.futex_atom.load(Acquire);
        loop {
            if atom.inbox == null() {
                break;
            } else {
                let new_atom = RawFutexAtom { inbox: null(), ..atom };
                if self.cmpxchg_weak::<usize, _, _>(&mut atom, new_atom, AcqRelT, AcquireT) {
                    self.enqueue(state, atom.inbox);
                    break;
                }
            }
        }
    }

    unsafe fn cancel(&self, state: &mut FutexState, waiter: *const Waiter) {
        state.queues[waiter.queue()].remove(waiter);
    }

    unsafe fn enqueue(&self, state: &mut FutexState, stack: *const Waiter) {
        let mut list = WaiterList::from_stack(stack);
        while let Some(waiter) = list.pop_front() {
            waiter.futex().store(FutexOwner { first: null(), second: self }, Relaxed);
            state.queues[waiter.queue()].push_back(waiter);
        }
    }

    unsafe fn cmpxchg_weak<A: Packable<Raw=usize>, Success: IsOrderingT, Failure: IsLoadT>(&self, current: &mut RawFutexAtom, new: RawFutexAtom, success: Success, failure: Failure) -> bool {
        if self.futex_atom.cmpxchg_weak(current, new, Success::ORDERING, Failure::ORDERING) {
            test_println!("from {:?}\n  to {:?}", current.debug::<A> (), new.debug::<A> ());
            true
        } else {
            false
        }
    }
}

impl<'waiters, 'futex, A: Packable<Raw=usize>> FutexQueueGuard<'waiters, 'futex, A> {
    /// * `update`: Called with the old atom and a bool indicating if there are any `Waiter`s.
    ///
    /// Atomically updates the current atom while ensuring new `Waiter`s
    /// are placed in the appropriate queues.
    #[must_use = "Failure might be spurious, which should be handled."]
    pub fn cmpxchg_enqueue_weak(&mut self, current: &mut FutexAtom<A>, new: A, success: impl IsAcquireT, failure: impl IsLoadT) -> bool {
        unsafe {
            let new = RawFutexAtom { atom: A::encode(new), inbox: null() };
            if self.futex.raw.cmpxchg_weak::<usize, _, _>(&mut current.raw, new, success, failure) {
                if current.raw.inbox != null() {
                    self.futex.raw.enqueue(&mut *self.ref_mut, current.raw.inbox);
                }
                true
            } else {
                false
            }
        }
    }

    /// * `queue`: the index of the queue to access.
    ///
    /// Pops one `Waiter` off the queue, or returns None if there were no waiters during the last
    /// `fetch_update_enqueue` operation.
    pub fn pop(&mut self, queue: usize) -> Option<WaiterHandle<'waiters>> {
        unsafe {
            Some(WaiterHandle::new(self.ref_mut.queues[queue].pop_front()? as *mut Waiter))
        }
    }

    /// * `count`: The maximum number of `Waiter`s to retrieve (specify `usize::MAX` for no maximum).
    /// * `queue`: The index of the queue to access.
    ///
    /// Pops up to `count` `Waiter`s off the queue, ignoring `Waiter`s added after the
    /// last `fetch_update_enqueue` operation.
    /// `enqueue` operation.
    pub fn pop_many(&mut self, count: usize, queue: usize) -> WaiterHandleList<'waiters> {
        unsafe {
            test_println!("Queue is {:?}", self.ref_mut.queues);
            WaiterHandleList::new(self.ref_mut.queues[queue].pop_front_many(count))
        }
    }

    ///
    pub fn is_empty(&self, queue: usize) -> bool {
        unsafe {
            self.ref_mut.queues[queue].is_empty()
        }
    }

    pub(crate) unsafe fn unsafe_debug<'b>(&'b self) -> impl 'b + Debug {
        struct Imp<'a, 'waiters, 'futex, A: Packable<Raw=usize>>(&'a FutexQueueGuard<'waiters, 'futex, A>);
        impl<'a, 'waiters, 'futex, A: Packable<Raw=usize>> Debug for Imp<'a, 'waiters, 'futex, A> {
            fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
                unsafe {
                    f.debug_list().entries(self.0.ref_mut.queues.iter().map(|x| x as &dyn Debug)).finish()
                }
            }
        }
        Imp(self)
    }
}

/// While this is a read-lock, none of the methods on a `FutexLock` or a `Waiter` may be called
/// simultaneously. Synchronization is left to the caller.
impl<'futex, A: Packable<Raw=usize>> FutexWaitersGuard<'futex, A> {
    pub fn lock<'waiters>(&'waiters self) -> FutexQueueGuard<'waiters, 'futex, A> {
        FutexQueueGuard { futex: self.futex, ref_mut: self.read_guard.borrow_mut() }
    }
}

unsafe impl<A: Packable<Raw=usize>> Send for Futex<A> {}

unsafe impl<A: Packable<Raw=usize>> Sync for Futex<A> {}

unsafe impl Send for RawFutexAtom {}

unsafe impl Sync for RawFutexAtom {}