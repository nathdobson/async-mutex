use crate::atomic::Atomic;
use crate::waiter::{Waiter, WaiterList};
use std::mem::MaybeUninit;
use crate::cell::UnsafeCell;
use std::ptr::null;
use crate::sync::Mutex;
use crate::sync::MutexGuard;
use crate::future::Future;
use std::task::{Context, Poll, Waker};
use std::pin::Pin;
use crate::sync::atomic::Ordering;
use crate::state::{FutexAtom, CopyWaker, WaiterWaker};
use crate::sync::atomic::Ordering::Acquire;
use crate::sync::atomic::Ordering::AcqRel;
use std::mem;
use crate::sync::RwLockReadGuard;
use crate::sync::RwLock;
use crate::sync::atomic::Ordering::Relaxed;
use crate::test_println;

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

pub struct EnterResult {
    pub(crate) userdata: Option<usize>,
    pub(crate) pending: bool,
}

pub struct WaitImpl<Enter, Exit> { pub(crate) enter: Enter, pub(crate) exit: Exit }

pub trait Wait {
    type Output;
    fn enter(&mut self, userdata: usize) -> EnterResult;
    fn exit(self) -> Self::Output;
}

impl<Enter: FnMut(usize) -> EnterResult, Exit: FnOnce<()>> Wait for WaitImpl<Enter, Exit> {
    type Output = Exit::Output;
    fn enter(&mut self, userdata: usize) -> EnterResult { (self.enter)(userdata) }
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
    pub fn new(userdata: usize) -> Self {
        Futex {
            atom: Atomic::new(FutexAtom { userdata, inbox: null() }),
            state: RwLock::new(UnsafeCell::new(FutexState {
                queue: WaiterList::new()
            })),
        }
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

    // Must be called sequentially
    pub unsafe fn pop_or_update<F>(
        &self,
        state: &mut FutexState,
        mut update: F,
    ) -> Option<*const Waiter>
        where F: FnMut(usize) -> Option<usize>
    {
        if let Some(waiter) = state.queue.pop() {
            return Some(waiter);
        }
        let mut atom = self.atom.load(Acquire);
        loop {
            if atom.inbox == null() {
                if let Some(userdata) = update(atom.userdata) {
                    if self.atom.cmpxchg_weak(&mut atom, FutexAtom { userdata, inbox: null() }, AcqRel, Acquire) {
                        return None;
                    }
                } else {
                    return None;
                }
            } else {
                let new_atom = FutexAtom { inbox: null(), ..atom };
                if self.atom.cmpxchg_weak(&mut atom, new_atom, AcqRel, Acquire) {
                    state.queue.append_stack(atom.inbox);
                    atom.inbox = null();
                    return Some(state.queue.pop().unwrap());
                }
            }
        }
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
                    state.queue.append_stack(atom.inbox);
                    atom.inbox = null();
                    break;
                }
            }
        }
        state.queue.remove(waiter);
    }

    unsafe fn requeue(&self, waiter: *const Waiter) {
        todo!()
    }
}

impl<'a, W: Wait> WaitFuture<'a, W> {
    unsafe fn poll_enter(&mut self, cx: &mut Context) -> Poll<W::Output> {
        let mut atom = self.original_futex.atom.load(Acquire);
        //TODO(skip this once)
        let waker = CopyWaker::from_waker(cx.waker().clone());
        loop {
            let result = self.wait.as_mut().unwrap().enter(atom.userdata);
            if result.pending {
                let waiter = self.waiter.as_mut_ptr();
                (waiter as *const Waiter).set_prev(atom.inbox);
                waiter.waker_mut().store_mut(WaiterWaker::Waiting { waker });
                waiter.futex_mut().store_mut(self.original_futex);
                let new_atom = FutexAtom { inbox: waiter, userdata: result.userdata.unwrap_or(atom.userdata) };
                if self.original_futex.atom.cmpxchg_weak(&mut atom, new_atom, AcqRel, Acquire) {
                    self.step = WaitFutureStep::Waiting;
                    return Poll::Pending;
                }
            } else {
                if let Some(userdata) = result.userdata {
                    let new_atom = FutexAtom { userdata, ..atom };
                    if self.original_futex.atom.cmpxchg_weak(&mut atom, new_atom, AcqRel, Acquire) {
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
                    let mut futex = self.waiter.as_ptr().futex().load(Relaxed);
                    while (*futex).state.write().unwrap().with_mut(|state| {
                        let futex2 = self.waiter.as_ptr().futex().load(Relaxed);
                        if futex == futex2 {
                            (*futex).cancel(&mut *state, self.waiter.as_ptr());
                            false
                        } else {
                            futex = futex2;
                            true
                        }
                    }) {}
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
        test_println!("Dropping {:?}", self.waiter.as_ptr());
    }
}

unsafe impl<'a, W: Wait> Send for WaitFuture<'a, W> {}

unsafe impl Send for Futex {}

unsafe impl Sync for Futex {}