use std::task::{RawWakerVTable, Waker};
use crate::futex::waiter::Waiter;
use std::{mem, ptr, fmt};
use crate::futex::atomic::Packable;
use crate::futex::atomic_impl::{AtomicUsize2, usize2};
use std::ptr::null;
use std::mem::align_of;
use std::fmt::{Debug, Formatter};
use crate::sync::atomic::AtomicUsize;

#[derive(Copy, Clone, Debug, Eq, PartialOrd, PartialEq, Ord)]
pub struct CopyWaker(pub *const (), pub *const RawWakerVTable);

#[derive(Copy, Clone, Debug, Eq, PartialOrd, PartialEq, Ord)]
pub enum WaiterWaker {
    None,
    Waiting { waker: CopyWaker },
    Done,
}

#[derive(Eq, Ord, PartialOrd, PartialEq)]
pub struct FutexAtom<M> {
    pub userdata: usize,
    pub inbox: *const Waiter<M>,
}

impl CopyWaker {
    pub unsafe fn from_waker(x: Waker) -> Self { mem::transmute(x) }
    pub unsafe fn into_waker(self) -> Waker { mem::transmute(self) }
}

pub const PANIC_WAKER_VTABLE: RawWakerVTable = RawWakerVTable::new(|_| panic!(), |_| panic!(), |_| panic!(), |_| panic!());
static WAITER_NONE_TAG: RawWakerVTable = PANIC_WAKER_VTABLE;
static WAITER_DONE_TAG: RawWakerVTable = PANIC_WAKER_VTABLE;

impl Packable for WaiterWaker {
    //type Impl = AtomicUsize2;
    type Raw = usize2;
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

impl<M> Packable for FutexAtom<M> {
    // type Impl = AtomicUsize2;
    type Raw = usize2;
    unsafe fn encode(val: Self) -> usize2 {
        mem::transmute(val)
    }

    unsafe fn decode(val: usize2) -> Self {
        mem::transmute(val)
    }
}

impl<M> Copy for FutexAtom<M> {}

impl<M> Clone for FutexAtom<M> {
    fn clone(&self) -> Self { *self }
}

impl<M> Debug for FutexAtom<M> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("FutexAtom")
            .field("userdata", &self.userdata)
            .field("inbox", &self.inbox)
            .finish()
    }
}

#[cfg(test)]
mod test {
    use crate::futex::atomic::Packable;
    use std::fmt::Debug;
    use crate::futex::state::{WaiterWaker, PANIC_WAKER_VTABLE, CopyWaker, FutexAtom};
    use std::task::RawWakerVTable;
    use crate::futex::waiter::{Waiter, WaiterList};
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
        let waiter: MaybeUninit<Waiter<()>> = MaybeUninit::uninit();
        verify(FutexAtom { userdata: 123, inbox: waiter.as_ptr() });
    }
}