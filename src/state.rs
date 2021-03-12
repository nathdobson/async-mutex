use std::task::{RawWakerVTable, Waker};
use crate::waiter::Waiter;
use std::{mem, ptr};
use crate::atomic::Packable;
use crate::atomic_impl::{AtomicUsize2, usize2};
use std::ptr::null;
use std::mem::align_of;
use std::fmt::Debug;

#[derive(Copy, Clone, Debug, Eq, PartialOrd, PartialEq, Ord)]
pub struct CopyWaker(pub *const (), pub *const RawWakerVTable);

#[derive(Copy, Clone, Debug, Eq, PartialOrd, PartialEq, Ord)]
pub enum WaiterWaker {
    None,
    Waiting { waker: CopyWaker },
    Done,
}

#[derive(Copy, Clone, Eq, Ord, PartialOrd, PartialEq, Debug)]
pub struct FutexAtom {
    pub userdata: usize,
    pub inbox: *const Waiter,
}

impl CopyWaker {
    pub unsafe fn from_waker(x: Waker) -> Self { mem::transmute(x) }
    pub unsafe fn into_waker(self) -> Waker { mem::transmute(self) }
}

const PANIC_WAKER_VTABLE: RawWakerVTable = RawWakerVTable::new(|_| panic!(), |_| panic!(), |_| panic!(), |_| panic!());
static WAITER_NONE_TAG: RawWakerVTable = PANIC_WAKER_VTABLE;
static WAITER_DONE_TAG: RawWakerVTable = PANIC_WAKER_VTABLE;

impl Packable for WaiterWaker {
    type Impl = AtomicUsize2;
    unsafe fn encode(val: Self) -> usize2 {
        mem::transmute(match val {
            WaiterWaker::None => CopyWaker(null(), &WAITER_NONE_TAG),
            WaiterWaker::Done => CopyWaker(null(), &WAITER_DONE_TAG),
            WaiterWaker::Waiting { waker } => waker,
        })
    }
    unsafe fn decode(val: usize2) -> Self {
        let waker: CopyWaker = mem::transmute(val);
        if waker.1 == &WAITER_NONE_TAG {
            WaiterWaker::None
        } else if waker.1 == &WAITER_DONE_TAG {
            WaiterWaker::Done
        } else {
            WaiterWaker::Waiting { waker }
        }
    }
}

impl Packable for FutexAtom {
    type Impl = AtomicUsize2;
    unsafe fn encode(val: Self) -> usize2 {
        mem::transmute(val)
    }

    unsafe fn decode(val: usize2) -> Self {
        mem::transmute(val)
    }
}

#[cfg(test)]
mod test {
    use crate::atomic::Packable;
    use std::fmt::Debug;
    use crate::state::{WaiterWaker, PANIC_WAKER_VTABLE, CopyWaker, FutexAtom};
    use std::task::RawWakerVTable;
    use crate::waiter::{Waiter, WaiterList};
    use std::mem::MaybeUninit;
    use crate::futex::FutexState;

    fn verify<T: Packable + Eq + Debug>(x: T) {
        unsafe { assert_eq!(T::decode(T::encode(x)), x); }
    }

    static DATA_VALUE: () = ();
    static VTABLE_VALUE: RawWakerVTable = PANIC_WAKER_VTABLE;
    fn copy_waker() -> CopyWaker {
        CopyWaker(&DATA_VALUE, &VTABLE_VALUE)
    }

    #[test]
    fn test_waiter_waker() {
        verify(WaiterWaker::None);
        verify(WaiterWaker::Done);
        verify(WaiterWaker::Waiting { waker: copy_waker() });
    }

    #[test]
    fn test_futex_atom() {
        let waiter = MaybeUninit::uninit();
        verify(FutexAtom { userdata: 123, inbox: waiter.as_ptr() });
    }
}