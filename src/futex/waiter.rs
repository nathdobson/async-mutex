use crate::cell::UnsafeCell;
use crate::atomic::{Atomic, Packable, IsAcquireT};
use std::ptr::null;
use crate::futex::state::WaiterWaker;
use crate::futex::{Futex, FutexState, RawFutex};
use crate::thread::Thread;
use crate::sync::atomic::Ordering::AcqRel;
//use crate::test_println;
use std::fmt::{Debug, Formatter};
use std::{fmt, mem};
use crate::atomic::usize2;
use std::mem::MaybeUninit;
use std::process::abort;
use std::marker::PhantomData;
use std::ops::Deref;

#[derive(Debug, Eq, PartialEq)]
pub(in crate::futex) struct FutexOwner {
    pub first: *const RawFutex,
    pub second: *const RawFutex,
}

pub struct Waiter {
    next: UnsafeCell<*const Waiter>,
    prev: UnsafeCell<*const Waiter>,
    futex: Atomic<FutexOwner>,
    queue: UnsafeCell<usize>,
    waker: Atomic<WaiterWaker>,
    message: usize,
}

pub(in crate::futex) struct WaiterList {
    pub head: *const Waiter,
    pub tail: *const Waiter,
}

#[must_use = "To avoid deadlock, wake or requeue."]
pub struct WaiterHandle<'waiters>(&'waiters mut Waiter);

#[must_use = "To avoid deadlock, wake or requeue."]
#[derive(Debug)]
pub struct WaiterHandleList<'waiters> {
    pub(in crate::futex) raw: WaiterList,
    phantom: PhantomData<&'waiters mut Waiter>,
}

pub struct WaiterHandleIterMut<'a> {
    pub(in crate::futex) raw: WaiterList,
    phantom: PhantomData<&'a mut Waiter>,
}

pub struct WaiterHandleIntoIter<'waiters> {
    pub(in crate::futex) raw: WaiterList,
    phantom: PhantomData<&'waiters mut Waiter>,
}

impl Waiter {
    pub(in crate::futex) fn new<A: Packable<Raw=usize>>(futex: &Futex<A>, message: usize, queue: usize) -> Self {
        Waiter {
            next: UnsafeCell::new(null()),
            prev: UnsafeCell::new(null()),
            futex: Atomic::new(FutexOwner { first: null(), second: &futex.raw }),
            queue: UnsafeCell::new(queue),
            waker: Atomic::new(WaiterWaker::None),
            message,
        }
    }
    pub(in crate::futex) unsafe fn waker<'a>(self: &'a *const Self) -> &'a Atomic<WaiterWaker> {
        &(**self).waker
    }
    pub(in crate::futex) unsafe fn waker_mut<'a>(self: &'a mut *mut Self) -> &'a mut Atomic<WaiterWaker> {
        &mut (**self).waker
    }
    pub(in crate::futex) unsafe fn futex<'a>(self: &'a *const Self) -> &'a Atomic<FutexOwner> {
        &(**self).futex
    }
    pub(in crate::futex) unsafe fn futex_mut<'a>(self: &'a mut *mut Self) -> &'a mut Atomic<FutexOwner> {
        &mut (**self).futex
    }
    pub(in crate::futex) unsafe fn message_raw(self: *const Self) -> usize {
        (*self).message
    }
    pub(in crate::futex) unsafe fn next(self: *const Self) -> *const Self {
        (*self).next.with_mut(|x| (*x))
    }
    pub(in crate::futex) unsafe fn prev(self: *const Self) -> *const Self {
        (*self).prev.with_mut(|x| (*x))
    }
    pub(in crate::futex) unsafe fn queue(self: *const Self) -> usize {
        (*self).queue.with_mut(|x| (*x))
    }
    pub(in crate::futex) unsafe fn set_next(self: *const Self, next: *const Self) {
        (*self).next.with_mut(|x| (*x) = next);
    }
    pub(in crate::futex) unsafe fn set_prev(self: *const Self, prev: *const Self) {
        (*self).prev.with_mut(|x| (*x) = prev);
    }
    pub(in crate::futex) unsafe fn set_queue(self: *const Self, queue: usize) {
        (*self).queue.with_mut(|x| (*x) = queue);
    }
    /// Cause the associated call to wait to return. Note that this method "takes ownership"
    /// of the waiter, and further calls on the waiter are undefined behavior.
    pub(in crate::futex) unsafe fn wake(self: *const Self) {
        match self.waker().swap(WaiterWaker::Done, AcqRel) {
            WaiterWaker::Waiting { waker } => {
                waker.into_waker().wake()
            }
            _ => panic!(),
        }
    }

    pub fn message(&self) -> usize {
        self.message
    }

}

impl<'a> WaiterHandle<'a> {
    pub(in crate::futex) unsafe fn new(waiter: *mut Waiter) -> Self {
        WaiterHandle(&mut *waiter)
    }
}

impl<'a> WaiterHandleList<'a> {
    pub(in crate::futex) unsafe fn new(waiter_list: WaiterList) -> Self {
        WaiterHandleList { raw: waiter_list, phantom: PhantomData }
    }
}

impl WaiterList {
    pub fn new() -> Self {
        WaiterList { head: null(), tail: null() }
    }
    /// Construct a doubly linked list starting with the tail and following prev pointers.
    pub(in crate::futex) unsafe fn from_stack(tail: *const Waiter) -> Self {
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
        //test_println!("Append {:?} {:?}", self, list);
        if self.head == null() && self.tail == null() {
            *self = list;
        } else {
            self.tail.set_next(list.head);
            list.head.set_prev(self.tail);
            self.tail = list.tail;
        }
        //test_println!("Append {:?}", self);
    }

    pub(in crate::futex) unsafe fn append_stack(&mut self, stack: *const Waiter) {
        self.append(Self::from_stack(stack));
    }

    /// Remove one link.´
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
        //test_println!("{:?}", self);
    }

    /// Remove from the head.
    pub unsafe fn pop_front(&mut self) -> Option<*const Waiter> {
        //test_println!("Popping {:?}", self.head);
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

    pub unsafe fn pop_front_many(&mut self, mut count: usize) -> WaiterList {
        if count == usize::MAX {
            return mem::replace(self, WaiterList::new());
        }
        let mut keep_head = self.head;
        while keep_head != null() && count > 0 {
            keep_head = keep_head.next();
            count -= 1;
        }
        if keep_head == null() {
            return mem::replace(self, WaiterList::new());
        }
        let ret_tail = keep_head.prev();
        if ret_tail == null() {
            return WaiterList::new();
        }
        let ret_head = self.head;
        self.head = keep_head;
        ret_tail.set_next(null());
        keep_head.set_prev(null());
        WaiterList { head: ret_head, tail: ret_tail }
    }

    // pub unsafe fn slice_split_1(&self) -> (*const Waiter, Self) {
    //     let new_head = self.head.next();
    //     if new_head == null() {
    //         (self.head, WaiterList::new())
    //     } else {
    //         (self.head, WaiterList { head: new_head, tail: self.tail })
    //     }
    // }

    pub unsafe fn push_back(&mut self, waiter: *const Waiter) {
        if self.head == null() && self.tail == null() {
            *self = WaiterList { head: waiter, tail: waiter };
        } else {
            self.tail.set_next(waiter);
            waiter.set_prev(self.tail);
            self.tail = waiter;
        }
    }
    pub unsafe fn is_empty(&self) -> bool {
        self.head == null()
    }
}

impl<'waiters> WaiterHandleList<'waiters> {
    pub fn is_empty(&self) -> bool {
        self.raw.head == null()
    }
}

impl<'waiters> WaiterHandle<'waiters> {
    pub fn wake(self) {
        unsafe {
            let ptr = self.0 as *const Waiter;
            mem::forget(self);
            ptr.wake();
        }
    }
}

impl<'waiters> Deref for WaiterHandle<'waiters> {
    type Target = Waiter;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl Drop for Waiter {
    fn drop(&mut self) {
        match self.waker.load_mut() {
            WaiterWaker::Waiting { waker } => {
                unsafe { mem::drop(waker.into_waker()) };
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

impl<'a> Iterator for WaiterHandleIterMut<'a> {
    type Item = &'a mut Waiter;

    fn next(&mut self) -> Option<Self::Item> {
        unsafe {
            if self.raw.head != null() {
                let next = self.raw.head.next();
                let old_head = mem::replace(&mut self.raw.head, next);
                if self.raw.head == null() {
                    self.raw.tail = null();
                }
                Some(&mut *(old_head as *mut Waiter))
            } else {
                None
            }
        }
    }
}

impl<'waiters> Iterator for WaiterHandleIntoIter<'waiters> {
    type Item = WaiterHandle<'waiters>;

    fn next(&mut self) -> Option<Self::Item> {
        unsafe {
            self.raw.pop_front().map(|x| WaiterHandle::new(x as *mut Waiter))
        }
    }
}

impl<'a, 'waiters> IntoIterator for &'a mut WaiterHandleList<'waiters> {
    type Item = &'a mut Waiter;
    type IntoIter = WaiterHandleIterMut<'a>;
    fn into_iter(self) -> Self::IntoIter {
        WaiterHandleIterMut { raw: self.raw, phantom: PhantomData }
    }
}

impl<'waiters> IntoIterator for WaiterHandleList<'waiters> {
    type Item = WaiterHandle<'waiters>;
    type IntoIter = WaiterHandleIntoIter<'waiters>;

    fn into_iter(self) -> Self::IntoIter {
        WaiterHandleIntoIter { raw: self.raw, phantom: PhantomData }
    }
}

impl Copy for FutexOwner {}

impl Clone for FutexOwner {
    fn clone(&self) -> Self { *self }
}

impl Packable for FutexOwner {
    type Raw = usize2;
    unsafe fn encode(val: Self) -> Self::Raw { mem::transmute(val) }
    unsafe fn decode(val: Self::Raw) -> Self { mem::transmute(val) }
}

impl Copy for WaiterList {}

impl Clone for WaiterList {
    fn clone(&self) -> Self { *self }
}

impl Default for WaiterList {
    fn default() -> Self {
        Self::new()
    }
}

unsafe impl Send for Waiter {}

unsafe impl Sync for Waiter {}