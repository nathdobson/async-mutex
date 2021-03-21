use std::{fmt, mem};
use std::fmt::{Debug, Formatter};
//use crate::test_println;
use std::marker::PhantomData;
use std::mem::{MaybeUninit, ManuallyDrop};
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
pub use waiter::WaiterHandleIter;
pub use crate::futex::wait_future::WaitFuture;

use crate::atomic::{Atomic, CopyWaker, Packable, IsOrderingT, AcqRelT, AcquireT, IsAcquireT, IsReleaseT, IsLoadT};
use crate::atomic::{AtomicUsize2, HasAtomic, IsAtomic, usize2, usize_half};
use crate::cell::UnsafeCell;
use crate::futex::atomic_refcell::{AtomicRefCell, RefMut};
use crate::futex::state::{RawFutexAtom, WaiterWaker};
use crate::futex::waiter::{WaiterList};
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
/// "multi-waiter, single-notifier" primitive. `wait` operations may occur simultaneously with any
/// operation. `wake`/`requeue` operations that occur simultaneously with other
/// `wake`/`requeue` operations may panic. For a "multi-notifier single-waiter" primitive, see
/// `futures::task::AtomicWaker`.
///
/// Waiters are stored in an array of queues, with strict FIFO behavior within each queue. Waiters
/// select which queue they want, and the waker may decide which queue to draw from.
///
/// There are no spurious wakeups: a `wait` operation will only return after a `wake` operation.
///
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

    /// Atomically retrieve the state.
    pub fn load<O: IsOrderingT>(&self, order: O) -> FutexAtom<A> {
        unsafe {
            FutexAtom { raw: self.raw.futex_atom.load(O::ORDERING), phantom: PhantomData }
        }
    }

    #[must_use = "Failure might be spurious, which should be handled."]
    /// Store a new value for the atom if the current value is the same as the `current` value.
    /// Otherwise updates `current` with the current value. Returns true on success. May fail spuriously.
    ///
    /// Arguments:
    ///
    /// * `current`: the expected current state.
    /// * `new`: the new atom.
    /// * `success`: the memory ordering on success.
    /// * `failure`: the memory ordering on failure.
    pub fn cmpxchg_weak(&self, current: &mut FutexAtom<A>, new: A, success: impl IsOrderingT, failure: impl IsLoadT) -> bool {
        unsafe {
            let new = RawFutexAtom { atom: A::encode(new), inbox: current.raw.inbox };
            self.raw.cmpxchg_weak::<A, _, _>(&mut current.raw, new, success, failure)
        }
    }

    /// Construct a new waiter for `cmpxchg_wait_weak`. Should typically be called outside a
    /// `cmpxchg_wait_weak` loop.
    ///
    /// Arguments:
    ///
    /// * `message`: a usize available to the task calling `wake`.
    /// * `queue`: the index of the queue to place this waiter.
    pub fn waiter(&self, message: usize, queue: usize) -> Waiter {
        Waiter::new(self, message, queue)
    }

    /// Like `cmpxchg_weak` except will also block asynchronously on success. Note that the `success`
    /// memory order controls the operation that begins blocking. The unblocking operation does
    /// not participate in the memory ordering of the `Futex`, but does "synchronize-with" the call
    /// to `wake` in both directions.
    ///
    /// Arguments:
    ///
    /// * `waiter`: The `Waiter` that will be placed in a queue.
    /// * `current`: The expected current state.
    /// * `new`: The new atom.
    /// * `success`: The memory ordering of the operation that begins to block (`ReleaseT`, `AcqRelT` or `SeqCstT`).
    /// * `failure`: The memory ordering of a failure (`RelaxedT` or `AcquireT`).
    /// * `on_pending`: Called immediately after the successful block operation but before yielding.
    /// * `on_cancel_woken`: Called if this future is canceled after `wake` is called on its `Waiter`.
    /// * `on_cancel_sleeping`: Called if this future is canceled before `wake` is called on its `Waiter`.
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

    /// Like `cmpxchg_wait_weak` except the parameters and return type are concrete types instead of `impl T`.
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
            assert!((waiter.as_mut().get_unchecked_mut() as *mut Waiter).futex_mut().load_mut() == &self.raw);
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

    /// Obtain a read lock in order to manipulate `Waiter`s associated with this `Futex`.
    /// Cancellation obtains a write lock, so this operation may block if cancellation is occurring.
    pub fn lock<'futex>(&'futex self) -> FutexWaitersGuard<'futex, A> {
        FutexWaitersGuard {
            futex: self,
            read_guard: self.raw.state.read().unwrap(),
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
        test_println!("Canceling!");
        state.queues[waiter.queue()].remove(waiter);
    }

    unsafe fn enqueue(&self, state: &mut FutexState, stack: *const Waiter) {
        let mut list = WaiterList::from_stack(stack);
        while let Some(waiter) = list.pop_front() {
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
    /// Like `cmpxchg_weak` except that `cmpxchg_wait_weak` operations earlier in the modification
    /// order of this `Futex` will have their `Waiter`s placed in the appropriate queues.
    ///
    /// Arguments:
    ///
    /// * `current`: The expected state.
    /// * `new`: The new atom.
    /// * `success`: The ordering of a successful operation (`AcquireT`, `AcqRelT`, or `SeqCst`).
    /// * `failure`: The ordering of a failure (`RelaxedT` or `AcquireT`).
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

    pub fn pop(&mut self, queue: usize) -> Option<WaiterHandle<'waiters>> {
        unsafe {
            Some(WaiterHandle::new(self.ref_mut.queues[queue].pop_front()? as *mut Waiter))
        }
    }

    pub fn pop_many(&mut self, count: usize, queue: usize) -> WaiterHandleList<'waiters> {
        unsafe {
            test_println!("Queue is {:?}", self.ref_mut.queues);
            WaiterHandleList::new(self.ref_mut.queues[queue].pop_front_many(count))
        }
    }

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

impl<'futex, A: Packable<Raw=usize>> FutexWaitersGuard<'futex, A> {
    /// Obtain an exclusive lock in order to manipulate the queues. If multiple threads attempt
    /// to acquire this lock, it may cause a panic.
    pub fn lock<'waiters>(&'waiters self) -> FutexQueueGuard<'waiters, 'futex, A> {
        FutexQueueGuard { futex: self.futex, ref_mut: self.read_guard.borrow_mut() }
    }

    /// Atomically transfer a list of waiters from another `Futex` to this `Futex`, as if
    /// `cmpxchg_wait_weak` had been called on this `Futex`. This is unsafe because the caller
    /// must ensure this `Futex` outlives the waiters without moving.
    pub unsafe fn requeue<'other_futex>(&self, mut list: WaiterHandleList<'other_futex>) {
        unsafe {
            if list.is_empty() {
                return;
            }
            for waiter in &list {
                (waiter as *const Waiter).futex().store(&self.futex.raw, Relaxed);
            }
            let mut atom = self.futex.raw.futex_atom.load(Acquire);
            let list = list.raw;
            loop {
                list.head.set_prev(atom.inbox);
                let new_atom = RawFutexAtom { atom: atom.atom, inbox: list.tail };
                if self.futex.raw.cmpxchg_weak::<usize, _, _>(
                    &mut atom, new_atom, AcqRelT, AcquireT) {
                    return;
                }
            }
        }
    }
}

unsafe impl<A: Packable<Raw=usize>> Send for Futex<A> {}

unsafe impl<A: Packable<Raw=usize>> Sync for Futex<A> {}

unsafe impl Send for RawFutexAtom {}

unsafe impl Sync for RawFutexAtom {}