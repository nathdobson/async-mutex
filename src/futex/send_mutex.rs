#![allow(deprecated)]

use crate::cell::UnsafeCell;
use dispatch::ffi;
use std::ops::{Index, Deref, DerefMut};

#[derive(Debug)]
pub struct SendMutex<T> {
    sem: Box<ffi::dispatch_semaphore_t>,
    cell: UnsafeCell<T>,
}

pub struct SendMutexGuard<'a, T: 'a> {
    mutex: &'a SendMutex<T>
}

impl<T> SendMutex<T> {
    pub fn new(inner: T) -> Self {
        unsafe {
            SendMutex {
                sem: Box::new(ffi::dispatch_semaphore_create(1)),
                cell: UnsafeCell::new(inner),
            }
        }
    }
    pub fn lock(&self) -> SendMutexGuard<T> {
        unsafe {
            ffi::dispatch_semaphore_wait(*self.sem, ffi::DISPATCH_TIME_FOREVER);
            SendMutexGuard { mutex: self }
        }
    }
    pub unsafe fn unforget_guard(&self) -> SendMutexGuard<T> {
        SendMutexGuard { mutex: self }
    }
}

impl<'a, T> Drop for SendMutexGuard<'a, T> {
    fn drop(&mut self) {
        unsafe {
            ffi::dispatch_semaphore_signal(*self.mutex.sem);
        }
    }
}

unsafe impl<T: Send> Send for SendMutex<T> {}

unsafe impl<T: Send> Sync for SendMutex<T> {}

unsafe impl<'a, T: Send> Send for SendMutexGuard<'a, T> {}

unsafe impl<'a, T: Send + Sync> Sync for SendMutexGuard<'a, T> {}

impl<'a, T> Deref for SendMutexGuard<'a, T> {
    type Target = T;
    fn deref(&self) -> &Self::Target {
        unsafe { self.mutex.cell.with_mut(|x| &*x) }
    }
}

impl<'a, T> DerefMut for SendMutexGuard<'a, T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        unsafe { self.mutex.cell.with_mut(|x| &mut *x) }
    }
}

#[cfg(test)]
mod test {
    use test::Bencher;
    use crate::futex::send_mutex::SendMutex;

    #[test]
    fn test_send_mutex() {
        let mutex = SendMutex::new(0);
    }

    #[bench]
    fn bench_send_mutex(b: &mut Bencher) {
        let mutex = SendMutex::new(10);
        b.iter(|| {
            *mutex.lock() += 1;
        });
    }

    #[bench]
    fn bench_mutex(b: &mut Bencher) {
        let mutex = std::sync::Mutex::new(10);
        b.iter(|| {
            *mutex.lock().unwrap() += 1;
        });
    }
}
