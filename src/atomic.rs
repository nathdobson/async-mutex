use crate::sync::atomic::{Ordering, AtomicUsize, AtomicBool, AtomicU128};
use std::marker::PhantomData;
use std::{mem};
use crate::atomic_impl::AtomicImpl;


/// An AtomicX containing a bitpacked `T` .
pub struct Atomic<T: Packable>(T::Impl, PhantomData<T>);

/// Specify how to bitpack a value.
pub trait Packable: Sized + Copy {
    type Impl: AtomicImpl;
    unsafe fn encode(val: Self) -> <Self::Impl as AtomicImpl>::Raw;
    unsafe fn decode(val: <Self::Impl as AtomicImpl>::Raw) -> Self;
}

impl<T: Packable> Atomic<T> {
    pub fn new(val: T) -> Self {
        Atomic(T::Impl::new(unsafe { T::encode(val) }), PhantomData)
    }
    pub fn load_mut(&mut self) -> T { unsafe { T::decode(self.0.load_mut()) } }
    pub fn store_mut(&mut self, value: T) { unsafe { self.0.store_mut(T::encode(value)); } }
    pub fn load(&self, order: Ordering) -> T {
        unsafe { T::decode(self.0.load(order)) }
    }
    pub fn store(&self, val: T, order: Ordering) {
        unsafe { self.0.store(T::encode(val), order); }
    }
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

impl<T> Packable for *const T {
    type Impl = AtomicUsize;
    unsafe fn encode(val: Self) -> usize { mem::transmute(val) }
    unsafe fn decode(val: usize) -> Self { mem::transmute(val) }
}

impl<T> Packable for *mut T {
    type Impl = AtomicUsize;
    unsafe fn encode(val: Self) -> usize { mem::transmute(val) }
    unsafe fn decode(val: usize) -> Self { mem::transmute(val) }
}

impl Packable for bool {
    type Impl = AtomicBool;

    unsafe fn encode(val: Self) -> bool { val }
    unsafe fn decode(val: bool) -> Self { val }
}
