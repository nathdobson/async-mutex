use crate::futex2::Queue;
use crate::cell::UnsafeCell;
use crate::sync::atomic::AtomicUsize;
use crate::sync::atomic::Ordering::Acquire;
use crate::sync::atomic::Ordering::Relaxed;
use crate::atomic::Atomic;
use crate::future::poll_fn;
use std::ops::{Deref, DerefMut};
use crate::sync::atomic::Ordering::Release;
use crate::sync::atomic::Ordering::SeqCst;
use crate::sync::atomic::Ordering::AcqRel;

#[derive(Debug)]
pub struct Mutex<T> {
    atom: Atomic<usize>,
    queue: Queue,
    inner: UnsafeCell<T>,
}

#[derive(Debug)]
pub struct MutexGuard<'a, T> {
    pub(crate) mutex: &'a Mutex<T>,
}

impl<T> Mutex<T> {
    pub fn new(inner: T) -> Self {
        Mutex { atom: Atomic::new(0), queue: Queue::with_capacity(1), inner: UnsafeCell::new(inner) }
    }

    pub fn get_mut(&mut self) -> &mut T {
        unsafe { self.inner.with_mut(|x| &mut *x) }
    }

    pub async fn lock<'a>(&'a self) -> MutexGuard<'a, T> {
        let mut state = 0;
        if self.atom.cmpxchg(&mut state, 1, AcqRel, Acquire) {
            test_println!("Locked immediately");
            return MutexGuard { mutex: self };
        }
        loop {
            if state == 0 {
                if self.atom.cmpxchg_weak(&mut state, 2, AcqRel, Acquire) {
                    test_println!("Locked later");
                    return MutexGuard { mutex: self };
                }
            } else if state == 1 {
                if self.atom.cmpxchg_weak(&mut state, 2, AcqRel, Acquire) {
                    test_println!("Marked waiting");
                    state = 2;
                }
            } else if state == 2 {
                test_println!("Waiting");
                self.queue.wait_if(|| {
                    state = self.atom.load(Acquire);
                    state == 2
                }).await;
                state = 0;
                test_println!("Waited");
            }
        }
    }

    pub(crate) unsafe fn unlock(&self) {
        test_println!("Unlocking");
        if self.atom.swap(0, AcqRel) == 2 {
            test_println!("Notifying");
            self.queue.notify(1);
            test_println!("Notified");
        }
        test_println!("Unlocked");
    }
}

impl<'a, T> Drop for MutexGuard<'a, T> {
    fn drop(&mut self) {
        unsafe { self.mutex.unlock(); }
    }
}

unsafe impl<T: Send> Send for Mutex<T> {}

unsafe impl<T: Send> Sync for Mutex<T> {}

unsafe impl<'a, T: Send> Send for MutexGuard<'a, T> {}

unsafe impl<'a, T: Send + Sync> Sync for MutexGuard<'a, T> {}

impl<'a, T> Deref for MutexGuard<'a, T> {
    type Target = T;
    fn deref(&self) -> &Self::Target { unsafe { &*self.mutex.inner.with_mut(|x| x) } }
}

impl<'a, T> DerefMut for MutexGuard<'a, T> {
    fn deref_mut(&mut self) -> &mut Self::Target { unsafe { &mut *self.mutex.inner.with_mut(|x| x) } }
}

impl<T: Default> Default for Mutex<T> {
    fn default() -> Self {
        Mutex::new(T::default())
    }
}