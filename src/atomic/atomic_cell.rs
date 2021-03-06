use crate::sync::atomic::{Ordering, AtomicUsize, AtomicBool};
use std::marker::PhantomData;
use std::{mem, fmt};
use crate::atomic::{HasAtomic, IsAtomic};
use std::fmt::{Debug, Formatter};
use crate::sync::atomic::Ordering::Relaxed;
use std::task::{RawWakerVTable, Waker};

/// An Atomic container for a bitpacked `T`, implemented using native atomics.
pub struct Atomic<T>(<T::Raw as HasAtomic>::Impl, PhantomData<T>) where T: Packable, T::Raw: HasAtomic;

/// Specify how to bitpack a value (typically into an unsigned integer type).
pub trait Packable: Sized + Copy + Debug {
    type Raw;
    unsafe fn encode(val: Self) -> Self::Raw;
    unsafe fn decode(val: Self::Raw) -> Self;
}

impl<T> Atomic<T> where T: Packable, T::Raw: HasAtomic {
    pub fn new(val: T) -> Self {
        Atomic(<T::Raw as HasAtomic>::Impl::new(unsafe { T::encode(val) }), PhantomData)
    }

    pub fn load_mut(&mut self) -> T { unsafe { T::decode(self.0.load_mut()) } }

    pub fn store_mut(&mut self, value: T) { unsafe { self.0.store_mut(T::encode(value)); } }

    pub fn load(&self, order: Ordering) -> T { unsafe { T::decode(self.0.load(order)) } }

    pub fn store(&self, val: T, order: Ordering) { unsafe { self.0.store(T::encode(val), order); } }

    pub fn swap(&self, val: T, order: Ordering) -> T {
        unsafe { T::decode(self.0.swap(T::encode(val), order)) }
    }

    pub fn cmpxchg_weak(&self, current: &mut T, new: T, success: Ordering, failure: Ordering) -> bool {
        unsafe {
            match self.0.compare_exchange_weak(
                T::encode(*current), T::encode(new), success, failure) {
                Ok(_) => true,
                Err(next) => {
                    *current = T::decode(next);
                    false
                }
            }
        }
    }

    pub fn cmpxchg(&self, current: &mut T, new: T, success: Ordering, failure: Ordering) -> bool {
        unsafe {
            match self.0.compare_exchange(
                T::encode(*current), T::encode(new), success, failure) {
                Ok(_) => true,
                Err(next) => {
                    *current = T::decode(next);
                    false
                }
            }
        }
    }
}

impl<T> Debug for Atomic<T> where T: Packable + Debug, T::Raw: HasAtomic {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}", self.load(Relaxed))
    }
}

impl<T> Packable for *const T {
    //type Impl = AtomicUsize;
    type Raw = usize;
    unsafe fn encode(val: Self) -> usize { mem::transmute(val) }
    unsafe fn decode(val: usize) -> Self { mem::transmute(val) }
}

impl<T> Packable for *mut T {
    //type Impl = AtomicUsize;
    type Raw = usize;
    unsafe fn encode(val: Self) -> usize { mem::transmute(val) }
    unsafe fn decode(val: usize) -> Self { mem::transmute(val) }
}

impl Packable for bool {
    type Raw = bool;
    unsafe fn encode(val: Self) -> bool { val }
    unsafe fn decode(val: bool) -> Self { val }
}

impl Packable for usize {
    type Raw = usize;
    unsafe fn encode(val: Self) -> Self::Raw { val }
    unsafe fn decode(val: Self::Raw) -> Self { val }
}

impl Packable for u8 {
    type Raw = u8;
    unsafe fn encode(val: Self) -> Self::Raw { val }
    unsafe fn decode(val: Self::Raw) -> Self { val }
}

/// A `CopyWaker` Similar to `RawWaker`, but it implements `Copy` and uses a pointer to a `RawWakerVTable`.
#[derive(Copy, Clone, Debug, Eq, PartialOrd, PartialEq, Ord)]
pub struct CopyWaker(pub *const (), pub *const RawWakerVTable);
impl CopyWaker {
    pub unsafe fn from_waker(x: Waker) -> Self { mem::transmute(x) }
    pub unsafe fn into_waker(self) -> Waker { mem::transmute(self) }
}
pub const PANIC_WAKER_VTABLE: RawWakerVTable = RawWakerVTable::new(|_| panic!(), |_| panic!(), |_| panic!(), |_| panic!());
