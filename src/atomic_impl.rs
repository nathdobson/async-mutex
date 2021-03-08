use crate::sync::atomic::{Ordering, AtomicUsize, AtomicBool};
#[cfg(target_has_atomic = "128")]
use crate::sync::atomic::{AtomicU128};
use crate::sync::atomic::AtomicU32;

pub trait AtomicImpl {
    type Raw: Copy;
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


macro_rules! atomic_impl (
    ($imp:ty, $raw:ty) => {
        impl AtomicImpl for $imp {
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
    }
);

atomic_impl!(AtomicUsize, usize);
atomic_impl!(AtomicBool, bool);
#[cfg(target_has_atomic = "128")]
atomic_impl!(AtomicU128, u128);

#[cfg(target_pointer_width = "16")]
pub type AtomicUsize2 = AtomicU32;

#[cfg(target_pointer_width = "32")]
pub type AtomicUsize2 = AtomicU64;

#[cfg(target_pointer_width = "64")]
pub type AtomicUsize2 = AtomicU128;

#[allow(non_camel_case_types)]
pub type usize2 = <AtomicUsize2 as AtomicImpl>::Raw;