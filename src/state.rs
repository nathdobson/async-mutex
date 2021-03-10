use std::task::{RawWakerVTable, Waker};
use crate::waiter::Waiter;
use std::mem;
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
    Done { canceled: bool },
    Waiting { waker: CopyWaker, canceling: bool },
}

#[derive(Copy, Clone, Debug, Eq, PartialOrd, PartialEq, Ord)]
pub enum MutexWaker {
    None,
    Canceling,
    Waiting(CopyWaker),
}

#[derive(Copy, Clone, Eq, Ord, PartialOrd, PartialEq, Debug)]
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

const MUTEX_STATE_ENCODING_IS_VALID: usize = align_of::<RawWakerVTable>() - 2;

impl Packable for WaiterWaker {
    type Impl = AtomicUsize2;
    unsafe fn encode(val: Self) -> usize2 {
        mem::transmute::<(*const (), usize), usize2>(match val {
            WaiterWaker::None => (null(), &WAITER_NONE_TAG as *const RawWakerVTable as usize),
            WaiterWaker::Done { canceled: false } => (null(), &WAITER_LOCKED_TAG as *const RawWakerVTable as usize),
            WaiterWaker::Done { canceled: true } => (null(), &WAITER_CANCELED_TAG as *const RawWakerVTable as usize),
            WaiterWaker::Waiting { waker, canceling } =>
                (waker.0, waker.1 as *const RawWakerVTable as usize | canceling as usize),
        })
    }
    unsafe fn decode(val: usize2) -> Self {
        let (data, vtable): (*const (), usize) = mem::transmute(val);

        if vtable == &WAITER_NONE_TAG as *const RawWakerVTable as usize {
            WaiterWaker::None
        } else if vtable == &WAITER_LOCKED_TAG as *const RawWakerVTable as usize {
            WaiterWaker::Done { canceled: false }
        } else if vtable == &WAITER_CANCELED_TAG as *const RawWakerVTable as usize {
            WaiterWaker::Done { canceled: true }
        } else {
            let canceling = vtable & 1 == 1;
            let vtable = vtable & !1;
            WaiterWaker::Waiting {
                waker: CopyWaker(data, &*(vtable as *mut RawWakerVTable)),
                canceling,
            }
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


const WAITER_WAKER_ENCODING_IS_VALID: usize = align_of::<Waiter>() - 2;

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

#[cfg(test)]
mod test {
    use crate::atomic::Packable;
    use std::fmt::Debug;
    use crate::state::{WaiterWaker, PANIC_WAKER_VTABLE, CopyWaker, MutexWaker, MutexState};
    use std::task::RawWakerVTable;
    use crate::waiter::Waiter;
    use std::mem::MaybeUninit;

    fn verify<T: Packable + Eq + Debug>(x: T) {
        unsafe {
            assert_eq!(T::decode(T::encode(x)), x);
        }
    }

    static DATA_VALUE: () = ();
    static VTABLE_VALUE: RawWakerVTable = PANIC_WAKER_VTABLE;

    fn copy_waker() -> CopyWaker {
        CopyWaker(&DATA_VALUE, &VTABLE_VALUE)
    }

    #[test]
    fn test_waiter_waker() {
        verify(WaiterWaker::None);
        verify(WaiterWaker::Done { canceled: false });
        verify(WaiterWaker::Done { canceled: true });
        verify(WaiterWaker::Waiting { waker: copy_waker(), canceling: false });
        verify(WaiterWaker::Waiting { waker: copy_waker(), canceling: true });
    }

    #[test]
    fn test_mutex_waker() {
        verify(MutexWaker::None);
        verify(MutexWaker::Canceling);
        verify(MutexWaker::Waiting(copy_waker()));
    }

    #[test]
    fn test_mutex_state() {
        let tail = MaybeUninit::uninit();
        let tail = tail.as_ptr();
        let canceling = MaybeUninit::uninit();
        let canceling = canceling.as_ptr();
        verify(MutexState { locked: false, tail, canceling });
        verify(MutexState { locked: true, tail, canceling });
    }
}