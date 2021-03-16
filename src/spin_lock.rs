use crate::cell::UnsafeCell;
use crate::futex::atomic::Atomic;
use crate::sync::atomic::Ordering::{Acquire, Relaxed};
use crate::sync::atomic::AtomicBool;
use crate::sync::atomic::Ordering::Release;
use std::ops::{Deref, DerefMut};
use crate::util::yield_now;
use std::default::default;

pub struct Mutex<T> {
    state: AtomicBool,
    inner: UnsafeCell<T>,
}

pub struct MutexGuard<'a, T> {
    mutex: &'a Mutex<T>,
}

impl<T> Mutex<T> {
    pub fn new(inner: T) -> Self {
        Mutex { state: AtomicBool::new(false), inner: UnsafeCell::new(inner) }
    }
    pub async fn lock<'a>(&'a self) -> MutexGuard<'a, T> {
        loop {
            if self.state.compare_exchange_weak(false, true, Acquire, Relaxed).is_ok() {
                return MutexGuard { mutex: self }
            } else {
                yield_now().await;
            }
        }
    }
}

impl<'a, T> Drop for MutexGuard<'a, T> {
    fn drop(&mut self) {
        self.mutex.state.store(false, Release);
    }
}

impl<'a, T> Deref for MutexGuard<'a, T> {
    type Target = T;
    fn deref(&self) -> &Self::Target { unsafe { &*self.mutex.inner.with_mut(|x| x) } }
}

impl<'a, T> DerefMut for MutexGuard<'a, T> {
    fn deref_mut(&mut self) -> &mut Self::Target { unsafe { &mut *self.mutex.inner.with_mut(|x| x) } }
}

unsafe impl<T: Send> Send for Mutex<T> {}

unsafe impl<T: Send> Sync for Mutex<T> {}

unsafe impl<'a, T: Send> Send for MutexGuard<'a, T> {}

unsafe impl<'a, T: Send + Sync> Sync for MutexGuard<'a, T> {}

impl<T:Default> Default for Mutex<T>{
    fn default() -> Self {
        Mutex::new(default())
    }
}