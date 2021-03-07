use std::task::{RawWakerVTable, Waker};
use crate::waiter::Waiter;
use std::mem;
use crate::atomic::Packable;
use crate::atomic_impl::{AtomicUsize2, usize2};
use std::ptr::null;
use std::mem::align_of;

#[derive(Copy, Clone)]
pub struct CopyWaker(pub *const (), pub &'static RawWakerVTable);

#[derive(Copy, Clone)]
pub enum WaiterWaker {
    None,
    Locked,
    Canceled,
    Waiting(CopyWaker),
}

#[derive(Copy, Clone)]
pub enum MutexWaker{
    None,
    Canceling,
    Waiting(CopyWaker),
}

#[derive(Copy, Clone, Eq, Ord, PartialOrd, PartialEq)]
pub struct MutexState {
    pub locked: bool,
    pub tail: *const Waiter,
    pub canceling: *const Waiter,
}

impl CopyWaker {
    pub unsafe fn from_waker(x: Waker) -> Self { mem::transmute(x) }
    pub unsafe fn into_waker(self) -> Waker { mem::transmute(self) }
}

const PANIC_WAKER_VTABLE: RawWakerVTable = RawWakerVTable::new(|_| panic!(), |_| panic!(), |_| panic!(), |_| panic!());
static WAITER_NONE_TAG: RawWakerVTable = PANIC_WAKER_VTABLE;
static WAITER_LOCKED_TAG: RawWakerVTable = PANIC_WAKER_VTABLE;
static WAITER_CANCELED_TAG: RawWakerVTable = PANIC_WAKER_VTABLE;
static MUTEX_NONE_TAG: RawWakerVTable = PANIC_WAKER_VTABLE;
static MUTEX_CANCELING_TAG: RawWakerVTable = PANIC_WAKER_VTABLE;

impl Packable for WaiterWaker {
    type Impl = AtomicUsize2;
    unsafe fn encode(val: Self) -> usize2 {
        mem::transmute(match val {
            WaiterWaker::None => CopyWaker(null(), &WAITER_NONE_TAG),
            WaiterWaker::Locked => CopyWaker(null(), &WAITER_LOCKED_TAG),
            WaiterWaker::Canceled => CopyWaker(null(), &WAITER_CANCELED_TAG),
            WaiterWaker::Waiting(waker) => waker,
        })
    }
    unsafe fn decode(val: usize2) -> Self {
        let waker: CopyWaker = mem::transmute(val);
        let ptr = waker.1 as *const RawWakerVTable;
        if ptr == &WAITER_NONE_TAG {
            WaiterWaker::None
        } else if ptr == &WAITER_LOCKED_TAG {
            WaiterWaker::Locked
        } else if ptr == &WAITER_CANCELED_TAG {
            WaiterWaker::Canceled
        } else {
            WaiterWaker::Waiting(waker)
        }
    }
}

impl Packable for MutexWaker {
    type Impl = AtomicUsize2;
    unsafe fn encode(val: Self) -> usize2 {
        mem::transmute(match val {
            MutexWaker::None => CopyWaker(null(), &MUTEX_NONE_TAG),
            MutexWaker::Canceling => CopyWaker(null(), &MUTEX_CANCELING_TAG),
            MutexWaker::Waiting(waker) => waker,
        })
    }
    unsafe fn decode(val: usize2) -> Self {
        let waker: CopyWaker = mem::transmute(val);
        let ptr = waker.1 as *const RawWakerVTable;
        if ptr == &MUTEX_NONE_TAG {
            MutexWaker::None
        } else if ptr == &MUTEX_CANCELING_TAG {
            MutexWaker::Canceling
        } else {
            MutexWaker::Waiting(waker)
        }
    }
}



const _: usize = align_of::<Waiter>() - 2;

impl Packable for MutexState {
    type Impl = AtomicUsize2;
    unsafe fn encode(val: Self) -> usize2 {
        mem::transmute((val.tail as usize, val.canceling as usize | val.locked as usize))
    }

    unsafe fn decode(val: usize2) -> Self {
        let (tail, canceling): (usize, usize) = mem::transmute(val);
        MutexState {
            locked: canceling & 1 == 1,
            tail: tail as *const Waiter,
            canceling: (canceling & !1) as *const Waiter,
        }
    }
}
