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

#[derive(Debug)]
pub struct FutexState {
    queue: WaiterList
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

pub struct EnterResult<A> {
    pub(crate) userdata: Option<A>,
    pub(crate) pending: bool,
}

pub struct WaitImpl<A, Enter, PostEnter, Exit> {
    pub enter: Enter,
    pub post_enter: Option<PostEnter>,
    pub exit: Exit,
    pub phantom: PhantomData<A>,
}

pub trait Wait {
    type Atom: Packable<Raw=usize>;
    type Output;
    fn enter(&mut self, userdata: Self::Atom) -> EnterResult<Self::Atom>;
    fn post_enter(&mut self);
    fn exit(self) -> Self::Output;
}

impl<A, Enter, PostEnter, Exit> Wait for WaitImpl<A, Enter, PostEnter, Exit> where
    A: Packable<Raw=usize>,
    Enter: FnMut(A) -> EnterResult<A>,
    PostEnter: FnOnce(),
    Exit: FnOnce<()>
{
    type Atom = A;
    type Output = Exit::Output;
    fn enter(&mut self, userdata: A) -> EnterResult<A> { (self.enter)(userdata) }
    fn post_enter(&mut self) { (self.post_enter.take().unwrap())(); }
    fn exit(self) -> Self::Output { (self.exit)() }
}

#[must_use = "with does nothing unless polled/`await`-ed"]
pub struct WaitFuture<'a, W: Wait> {
    original_futex: &'a Futex,
    step: WaitFutureStep,
    // TODO MaybeUninit isn't exactly right here: the wrapper should communicate to
    // the compiler that it is unsafe to convert a `&mut LockFuture` to a `&mut Waiter`,
    // while it is safe to convert an `&mut LockFuture` to a `&Waiter`.
    waiter: MaybeUninit<Waiter>,
    wait: Option<W>,
}

impl Futex {
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
    pub unsafe fn wait<W: Wait>(&self, wait: W) -> WaitFuture<W> {
        WaitFuture {
            original_futex: self,
            step: WaitFutureStep::Enter,
            waiter: MaybeUninit::new(Waiter::new()),
            wait: Some(wait),
        }
    }
    pub unsafe fn lock_state(&self) -> RwLockReadGuard<UnsafeCell<FutexState>> {
        self.state.read().unwrap()
    }

    /// Update userdata atomically. The bool indicates if the queue has waiters. Ensures the queue
    /// is flipped.
    pub unsafe fn update_flip<A, F>(
        &self,
        state: &mut FutexState,
        mut update: F,
    ) -> bool
        where A: Packable<Raw=usize>, F: FnMut(A, bool) -> Option<A>
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

    pub unsafe fn pop(&self, state: &mut FutexState) -> Option<*const Waiter> {
        state.queue.pop()
    }

    pub unsafe fn pop_many(&self, state: &mut FutexState, count: usize) -> WaiterList {
        test_println!("Queue is {:?}", state.queue);
        state.queue.pop_many(count)
    }

    unsafe fn cancel(&self, state: &mut FutexState, waiter: *const Waiter) {
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

impl<'a, W: Wait> WaitFuture<'a, W> {
    unsafe fn poll_enter(&mut self, cx: &mut Context) -> Poll<W::Output> {
        let mut atom = self.original_futex.atom.load(Acquire);
        //TODO(skip this once)
        let waker = CopyWaker::from_waker(cx.waker().clone());
        loop {
            let result = self.wait.as_mut().unwrap().enter(W::Atom::decode(atom.userdata));
            if result.pending {
                let waiter = self.waiter.as_mut_ptr();
                (waiter as *const Waiter).set_prev(atom.inbox);
                waiter.waker_mut().store_mut(WaiterWaker::Waiting { waker });
                waiter.futex_mut().store_mut(FutexOwner { first: null(), second: self.original_futex });
                let mut userdata =
                    if let Some(encoded) = result.userdata {
                        W::Atom::encode(encoded)
                    } else {
                        atom.userdata
                    };
                let new_atom = FutexAtom { inbox: waiter, userdata };
                if self.original_futex.atom.cmpxchg_weak(&mut atom, new_atom, AcqRel, Acquire) {
                    test_println!("{:?} -> {:?}", atom, new_atom);
                    self.wait.as_mut().unwrap().post_enter();
                    self.step = WaitFutureStep::Waiting;
                    return Poll::Pending;
                }
            } else {
                if let Some(userdata) = result.userdata {
                    let new_atom = FutexAtom { userdata: W::Atom::encode(userdata), ..atom };
                    if self.original_futex.atom.cmpxchg_weak(&mut atom, new_atom, AcqRel, Acquire) {
                        test_println!("{:?} -> {:?}", atom, new_atom);
                        mem::drop(waker.into_waker());
                        self.step = WaitFutureStep::Done;
                        return Poll::Ready(self.wait.take().unwrap().exit());
                    }
                } else {
                    mem::drop(waker.into_waker());
                    return Poll::Ready(self.wait.take().unwrap().exit());
                }
            }
        }
    }
    unsafe fn poll_waiting(&mut self, cx: &mut Context) -> Poll<W::Output> {
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
                    return Poll::Ready(self.wait.take().unwrap().exit());
                }
            }
        }
    }
}

impl<'a, W: Wait> Future for WaitFuture<'a, W> {
    type Output = W::Output;

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

impl<'a, W: Wait> Drop for WaitFuture<'a, W> {
    fn drop(&mut self) {
        unsafe {
            match mem::replace(&mut self.step, WaitFutureStep::Done) {
                WaitFutureStep::Enter => {
                    test_println!("Dropping on enter");
                }
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
                        WaiterWaker::Waiting { waker } => mem::drop(waker.into_waker()),
                        WaiterWaker::Done => {
                            test_println!("Completed before canceling");
                            mem::drop(self.wait.take().unwrap().exit())
                        }
                    }
                }
                WaitFutureStep::Done => {}
            }
        }
        //test_println!("Dropping {:?}", self.waiter.as_ptr());
    }
}

unsafe impl<'a, W: Wait> Send for WaitFuture<'a, W> {}

unsafe impl Send for Futex {}

unsafe impl Sync for Futex {}
