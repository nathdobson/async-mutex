use crate::cell::UnsafeCell;
use crate::atomic::Atomic;
use std::ptr::null;
use crate::state::WaiterWaker;
use crate::util::bad_cancel;
use crate::futex::Futex;
use crate::thread::Thread;
use crate::sync::atomic::Ordering::AcqRel;
use crate::test_println;
use std::fmt::{Debug, Formatter};
use std::fmt;

pub struct Waiter {
    next: UnsafeCell<*const Waiter>,
    prev: UnsafeCell<*const Waiter>,
    futex: Atomic<*const Futex>,
    waker: Atomic<WaiterWaker>,
}

pub struct WaiterList {
    pub head: *const Waiter,
    pub tail: *const Waiter,
}

impl Waiter {
    pub fn new() -> Self {
        Waiter {
            next: UnsafeCell::new(null()),
            prev: UnsafeCell::new(null()),
            futex: Atomic::new(null()),
            waker: Atomic::new(WaiterWaker::None),
        }
    }
    pub unsafe fn waker(self: *const Self) -> &'static Atomic<WaiterWaker> {
        &(*self).waker
    }
    pub unsafe fn waker_mut(self: *mut Self) -> &'static mut Atomic<WaiterWaker> {
        &mut (*self).waker
    }
    pub unsafe fn futex(self: *const Self) -> &'static Atomic<*const Futex> {
        &(*self).futex
    }
    pub unsafe fn futex_mut(self: *mut Self) -> &'static mut Atomic<*const Futex> {
        &mut (*self).futex
    }
    pub unsafe fn next(self: *const Self) -> *const Self {
        (*self).next.with_mut(|x| (*x))
    }
    pub unsafe fn prev(self: *const Self) -> *const Self {
        (*self).prev.with_mut(|x| (*x))
    }
    pub unsafe fn set_next(self: *const Self, next: *const Self) {
        (*self).next.with_mut(|x| (*x) = next);
    }
    pub unsafe fn set_prev(self: *const Self, prev: *const Self) {
        (*self).prev.with_mut(|x| (*x) = prev);
    }
    pub unsafe fn done(self: *const Self) {
        match self.waker().swap(WaiterWaker::Done, AcqRel) {
            WaiterWaker::Waiting { waker } => {
                waker.into_waker().wake()
            }
            _ => panic!(),
        }
    }
}

impl WaiterList {
    pub fn new() -> WaiterList {
        WaiterList { head: null(), tail: null() }
    }
    /// Construct a doubly linked list starting with the tail and prev pointers.
    pub unsafe fn from_stack(tail: *const Waiter) -> Self {
        let mut head = tail;
        if head != null() {
            loop {
                let prev = head.prev();
                if prev == null() {
                    break;
                } else {
                    prev.set_next(head);
                    head = prev;
                }
            }
        }
        WaiterList { head, tail }
    }
    pub unsafe fn append(&mut self, list: Self) {
        test_println!("Append {:?} {:?}", self, list);
        if self.head == null() && self.tail == null() {
            *self = list;
        } else {
            self.tail.set_next(list.head);
            list.head.set_prev(self.tail);
            self.tail = list.tail;
        }
        test_println!("Append {:?}", self);
    }

    pub unsafe fn append_stack(&mut self, stack: *const Waiter) {
        self.append(Self::from_stack(stack));
    }

    /// Remove one link.
    pub unsafe fn remove(&mut self, waiter: *const Waiter) {
        let prev = waiter.prev();
        let next = waiter.next();
        test_println!("Removing {:?} {:?} {:?} {:?}",self, waiter,prev,next);
        if prev == null() {
            if self.head == waiter {
                self.head = next;
            }
        } else {
            prev.set_next(next);
        }
        if next == null() {
            if self.tail == waiter {
                self.tail = prev;
            }
        } else {
            next.set_prev(prev);
        }
        test_println!("{:?}", self);
    }

    /// Remove from the head.
    pub unsafe fn pop(&mut self) -> Option<*const Waiter> {
        test_println!("Popping {:?}", self.head);
        if self.head == null() {
            None
        } else {
            let result = self.head;
            self.head = self.head.next();
            if self.tail == result {
                self.tail = null();
            }
            if result.next() != null() {
                result.next().set_prev(null());
            }
            result.set_next(null());
            Some(result)
        }
    }
}

impl Drop for Waiter {
    fn drop(&mut self) {
        match self.waker.load_mut() {
            WaiterWaker::Waiting { waker } => {
                bad_cancel();
            }
            _ => {}
        }
    }
}

impl Debug for WaiterList {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        unsafe {
            let mut list = f.debug_list();
            let mut node = self.head;
            if node == null() {
                assert_eq!(self.tail, null());
            } else {
                assert_eq!(node.prev(), null());
                loop {
                    list.entry(&node);
                    let next = node.next();
                    if next == null() {
                        assert_eq!(self.tail, node);
                        break;
                    } else {
                        assert_eq!(node, next.prev());
                    }
                    node = next;
                }
            }
            list.finish()?;
            Ok(())
        }
    }
}