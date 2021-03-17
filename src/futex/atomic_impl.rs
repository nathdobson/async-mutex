use crate::sync::atomic::{Ordering, AtomicUsize, AtomicBool};
#[cfg(target_has_atomic = "8")]
use crate::sync::atomic::AtomicU8;
#[cfg(target_has_atomic = "16")]
use crate::sync::atomic::AtomicU16;
#[cfg(target_has_atomic = "32")]
use crate::sync::atomic::AtomicU32;
#[cfg(target_has_atomic = "64")]
use crate::sync::atomic::AtomicU64;
#[cfg(target_has_atomic = "128")]
use crate::sync::atomic::AtomicU128;
use std::fmt::Debug;

/// Atomic unsigned integers (AtomicU8, AtomicUsize, etc.)
pub trait IsAtomic: Debug {
    type Raw: Copy + HasAtomic<Impl=Self> + Debug;
    fn new(x: Self::Raw) -> Self;
    fn load_mut(&mut self) -> Self::Raw;
    fn store_mut(&mut self, raw: Self::Raw);
    fn load(&self, order: Ordering) -> Self::Raw;
    fn store(&self, val: Self::Raw, order: Ordering);
    fn swap(&self, val: Self::Raw, order: Ordering) -> Self::Raw;

    fn compare_exchange(
        &self,
        current: Self::Raw, new: Self::Raw,
        success: Ordering, failure: Ordering,
    ) -> Result<Self::Raw, Self::Raw>;

    fn compare_exchange_weak(
        &self,
        current: Self::Raw, new: Self::Raw,
        success: Ordering, failure: Ordering,
    ) -> Result<Self::Raw, Self::Raw>;
}

/// Unsigned integers for which an atomic type exists.
pub trait HasAtomic: Debug {
    type Impl: IsAtomic<Raw=Self> + Debug;
}

macro_rules! atomic_impl (
    ($imp:ty, $raw:ty) => {
        impl IsAtomic for $imp {
            type Raw = $raw;
            fn new(x: Self::Raw) -> Self { Self::new(x) }
            fn load_mut(&mut self) -> Self::Raw {
                #[cfg(loom)]
                return unsafe { self.unsync_load() };
                #[cfg(not(loom))]
                return *self.get_mut();
            }
            fn store_mut(&mut self, raw: Self::Raw) {
                #[cfg(loom)]
                { *self = Self::new(raw); }
                #[cfg(not(loom))]
                { *self.get_mut() = raw; }
            }
            fn load(&self, order: Ordering) -> Self::Raw { self.load(order) }
            fn store(&self, val: Self::Raw, order: Ordering) { self.store(val, order) }
            fn swap(&self, val: Self::Raw, order: Ordering) -> Self::Raw { self.swap(val, order) }

            fn compare_exchange(
                &self,
                current: Self::Raw, new: Self::Raw,
                success: Ordering, failure: Ordering,
            ) -> Result<Self::Raw, Self::Raw> {
                self.compare_exchange(current, new, success, failure)
            }

            fn compare_exchange_weak(
                &self,
                current: Self::Raw, new: Self::Raw,
                success: Ordering, failure: Ordering,
            ) -> Result<Self::Raw, Self::Raw> {
                self.compare_exchange_weak(current, new, success, failure)
            }
        }
        impl HasAtomic for $raw {
            type Impl = $imp;
        }
    }
);



atomic_impl!(AtomicUsize, usize);
atomic_impl!(AtomicBool, bool);
#[cfg(target_has_atomic = "8")]
atomic_impl!(AtomicU8, u8);
#[cfg(target_has_atomic = "16")]
atomic_impl!(AtomicU16, u16);
#[cfg(target_has_atomic = "32")]
atomic_impl!(AtomicU32, u32);
#[cfg(target_has_atomic = "64")]
atomic_impl!(AtomicU64, u64);
#[cfg(target_has_atomic = "128")]
atomic_impl!(AtomicU128, u128);

pub struct PlatformDoesNotSupportDoubleWideCompareAndSwap;

#[cfg(all(target_pointer_width = "16", target_has_atomic = "32"))]
pub type AtomicUsize2 = AtomicU32;

#[cfg(all(target_pointer_width = "16", not(target_has_atomic = "32")))]
pub type AtomicUsize2 = [PlatformDoesNotSupportDoubleWideCompareAndSwap; 0 - 1];

#[cfg(all(target_pointer_width = "32", target_has_atomic = "64"))]
pub type AtomicUsize2 = AtomicU64;

#[cfg(all(target_pointer_width = "32", not(target_has_atomic = "64")))]
pub type AtomicUsize2 = [PlatformDoesNotSupportDoubleWideCompareAndSwap; 0 - 1];

#[cfg(all(target_pointer_width = "64", target_has_atomic = "128"))]
pub type AtomicUsize2 = AtomicU128;

#[cfg(all(target_pointer_width = "64", not(target_has_atomic = "128")))]
pub type AtomicUsize2 = [PlatformDoesNotSupportDoubleWideCompareAndSwap; 0 - 1];

#[allow(non_camel_case_types)]
pub type usize2 = <AtomicUsize2 as IsAtomic>::Raw;

#[allow(non_camel_case_types)]
#[cfg(all(target_pointer_width = "16"))]
pub type usize_half = u8;

#[allow(non_camel_case_types)]
#[cfg(all(target_pointer_width = "32"))]
pub type usize_half = u16;

#[allow(non_camel_case_types)]
#[cfg(all(target_pointer_width = "64"))]
pub type usize_half = u32;

