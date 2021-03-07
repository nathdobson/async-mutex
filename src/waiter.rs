use std::cell::UnsafeCell;
use crate::atomic::Atomic;
use std::ptr::null;
use crate::state::WaiterWaker;
use crate::bad_cancel;

pub struct Waiter {
    pub next: UnsafeCell<*const Waiter>,
    pub prev: UnsafeCell<*const Waiter>,
    pub next_canceling: UnsafeCell<*const Waiter>,
    pub waker: Atomic<WaiterWaker>,
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
}

impl Drop for Waiter {
    fn drop(&mut self) {
        match self.waker.load_mut() {
            WaiterWaker::Waiting(waker) => {
                bad_cancel();
            }
            _ => {}
        }
    }
}
