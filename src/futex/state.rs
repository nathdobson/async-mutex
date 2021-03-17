use std::task::{RawWakerVTable, Waker};
use crate::futex::waiter::Waiter;
use std::{mem, ptr, fmt};
use crate::futex::atomic::Packable;
use crate::futex::atomic_impl::{AtomicUsize2, usize2};
use std::ptr::null;
use std::mem::align_of;
use std::fmt::{Debug, Formatter};
use crate::sync::atomic::AtomicUsize;
use std::marker::PhantomData;

/// A `CopyWaker` Similar to `RawWaker`, but it implements `Copy` and uses a pointer to a `RawWakerVTable`.
#[derive(Copy, Clone, Debug, Eq, PartialOrd, PartialEq, Ord)]
pub struct CopyWaker(pub *const (), pub *const RawWakerVTable);

#[derive(Copy, Clone, Debug, Eq, PartialOrd, PartialEq, Ord)]
pub(in crate::futex) enum WaiterWaker {
    None,
    Waiting { waker: CopyWaker },
    Done,
}

#[derive(Eq, Ord, PartialOrd, PartialEq)]
pub(in crate::futex) struct FutexAtom {
    pub atom: usize,
    pub inbox: *const Waiter,
}

impl CopyWaker {
    pub unsafe fn from_waker(x: Waker) -> Self { mem::transmute(x) }
    pub unsafe fn into_waker(self) -> Waker { mem::transmute(self) }
}

/// It is possible to reserve a value of type `*const RawWakerVTable` by creating a static with value `PANIC_WAKER_VTABLE`.
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

impl FutexAtom {
    pub(crate) fn debug<A: Packable<Raw=usize> + Debug>(self) -> impl Debug {
        struct Result<A>(FutexAtom, PhantomData<A>);
        impl<A: Packable<Raw=usize> + Debug> Debug for Result<A> {
            fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
                unsafe {
                    f.debug_struct("FutexAtom")
                        .field("inbox", &format!("{: >16}", format!("{:p}", self.0.inbox)))
                        .field("atom", &A::decode(self.0.atom))
                        .finish()
                }
            }
        }
        Result::<A>(self, PhantomData)
    }
}

impl Packable for FutexAtom {
    // type Impl = AtomicUsize2;
    type Raw = usize2;
    unsafe fn encode(val: Self) -> usize2 {
        mem::transmute(val)
    }

    unsafe fn decode(val: usize2) -> Self {
        mem::transmute(val)
    }
}

impl Copy for FutexAtom {}

impl Clone for FutexAtom {
    fn clone(&self) -> Self { *self }
}

impl Debug for FutexAtom {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("FutexAtom")
            .field("atom", &self.atom)
            .field("inbox", &self.inbox)
            .finish()
    }
}

#[cfg(test)]
pub fn test_packable<T: Packable + Eq + Debug>(x: T) {
    unsafe { assert_eq!(T::decode(T::encode(x)), x); }
}


#[cfg(test)]
mod test {
    use crate::futex::atomic::Packable;
    use std::fmt::Debug;
    use crate::futex::state::{WaiterWaker, PANIC_WAKER_VTABLE, CopyWaker, FutexAtom, test_packable};
    use std::task::RawWakerVTable;
    use crate::futex::waiter::{Waiter, WaiterList};
    use std::mem::MaybeUninit;
    use crate::futex::FutexState;


    static DATA_VALUE: () = ();
    static VTABLE_VALUE: RawWakerVTable = PANIC_WAKER_VTABLE;

    fn copy_waker() -> CopyWaker {
        CopyWaker(&DATA_VALUE, &VTABLE_VALUE)
    }

    #[test]
    fn test_waiter_waker() {
        test_packable(WaiterWaker::None);
        test_packable(WaiterWaker::Done);
        test_packable(WaiterWaker::Waiting { waker: copy_waker() });
    }

    #[test]
    fn test_futex_atom() {
        let waiter: MaybeUninit<Waiter> = MaybeUninit::uninit();
        test_packable(FutexAtom { atom: 123, inbox: waiter.as_ptr() });
    }
}