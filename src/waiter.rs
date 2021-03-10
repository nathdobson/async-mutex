use crate::cell::UnsafeCell;
use crate::atomic::Atomic;
use std::ptr::null;
use crate::state::WaiterWaker;
use crate::bad_cancel;


pub struct Waiter {
    next: UnsafeCell<*const Waiter>,
    prev: UnsafeCell<*const Waiter>,
    next_canceling: UnsafeCell<*const Waiter>,
    waker: Atomic<WaiterWaker>,
}

impl Waiter {
    pub fn new() -> Self {
        Waiter {
            next: UnsafeCell::new(null()),
            prev: UnsafeCell::new(null()),
            next_canceling: UnsafeCell::new(null()),
            waker: Atomic::new(WaiterWaker::None),
        }
    }
    pub unsafe fn waker(self: *const Self) -> &'static Atomic<WaiterWaker> {
        &(*self).waker
    }
    pub unsafe fn waker_mut(self: *mut Self) -> &'static mut Atomic<WaiterWaker> {
        &mut (*self).waker
    }
    pub unsafe fn next(self: *const Self) -> *const Self {
        (*self).next.with_mut(|x| (*x))
    }
    pub unsafe fn prev(self: *const Self) -> *const Self {
        (*self).prev.with_mut(|x| (*x))
    }
    pub unsafe fn next_canceling(self: *const Self) -> *const Self {
        (*self).next_canceling.with_mut(|x| (*x))
    }
    pub unsafe fn set_next(self: *const Self, next: *const Self) {
        (*self).next.with_mut(|x| (*x) = next);
    }
    pub unsafe fn set_prev(self: *const Self, prev: *const Self) {
        (*self).prev.with_mut(|x| (*x) = prev);
    }
    pub unsafe fn set_next_canceling(self: *const Self, next_canceling: *const Self) {
        (*self).next_canceling.with_mut(|x| (*x) = next_canceling);
    }
}

impl Drop for Waiter {
    fn drop(&mut self) {
        match self.waker.load_mut() {
            WaiterWaker::Waiting{ waker, canceling } => {
                bad_cancel();
            }
            _ => {}
        }
    }
}
