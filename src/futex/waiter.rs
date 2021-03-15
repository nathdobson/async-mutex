use crate::cell::UnsafeCell;
use crate::futex::atomic::{Atomic, Packable};
use std::ptr::null;
use crate::futex::state::WaiterWaker;
use crate::futex::{Futex, FutexState};
use crate::thread::Thread;
use crate::sync::atomic::Ordering::AcqRel;
use crate::test_println;
use std::fmt::{Debug, Formatter};
use std::{fmt, mem};
use crate::futex::atomic_impl::usize2;
use std::mem::MaybeUninit;
use std::process::abort;

#[derive(Debug)]
pub struct FutexOwner<M> {
    pub first: *const Futex<M>,
    pub second: *const Futex<M>,
}

pub struct Waiter<M> {
    next: UnsafeCell<*const Waiter<M>>,
    prev: UnsafeCell<*const Waiter<M>>,
    futex: Atomic<FutexOwner<M>>,
    waker: Atomic<WaiterWaker>,
    message: Option<UnsafeCell<M>>,
}

pub struct WaiterList<M> {
    pub head: *const Waiter<M>,
    pub tail: *const Waiter<M>,
}

impl<M> Waiter<M> {
    pub fn new(message: M) -> Self {
        Waiter {
            next: UnsafeCell::new(null()),
            prev: UnsafeCell::new(null()),
            futex: Atomic::new(FutexOwner { first: null(), second: null() }),
            waker: Atomic::new(WaiterWaker::None),
            message: Some(UnsafeCell::new(message)),
        }
    }
    pub unsafe fn waker<'a>(self: &'a *const Self) -> &'a Atomic<WaiterWaker> {
        &(**self).waker
    }
    pub unsafe fn waker_mut<'a>(self: &'a mut *mut Self) -> &'a mut Atomic<WaiterWaker> {
        &mut (**self).waker
    }
    pub unsafe fn futex<'a>(self: &'a *const Self) -> &'a Atomic<FutexOwner<M>> {
        &(**self).futex
    }
    pub unsafe fn futex_mut<'a>(self: &'a mut *mut Self) -> &'a mut Atomic<FutexOwner<M>> {
        &mut (**self).futex
    }
    pub unsafe fn message<'a>(self: &'a *const Self) -> &'a UnsafeCell<M> {
        (**self).message.as_ref().unwrap()
    }
    pub(crate) unsafe fn take_message(self: &mut Self) -> M {
        self.message.take().unwrap().into_inner()
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

impl<M> WaiterList<M> {
    pub fn new() -> Self {
        WaiterList { head: null(), tail: null() }
    }
    /// Construct a doubly linked list starting with the tail and prev pointers.
    pub unsafe fn from_stack(tail: *const Waiter<M>) -> Self {
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

    pub unsafe fn append_stack(&mut self, stack: *const Waiter<M>) {
        self.append(Self::from_stack(stack));
    }

    /// Remove one link.´
    pub unsafe fn remove(&mut self, waiter: *const Waiter<M>) {
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
    pub unsafe fn pop(&mut self) -> Option<*const Waiter<M>> {
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

    pub unsafe fn empty(&self) -> bool {
        self.head == null()
    }

    pub unsafe fn pop_many(&mut self, mut count: usize) -> WaiterList<M> {
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

    pub unsafe fn slice_split_1(&self) -> (*const Waiter<M>, Self) {
        let new_head = self.head.next();
        if new_head == null() {
            (self.head, WaiterList::new())
        } else {
            (self.head, WaiterList { head: new_head, tail: self.tail })
        }
    }
}

impl<M> Drop for Waiter<M> {
    fn drop(&mut self) {
        match self.waker.load_mut() {
            WaiterWaker::Waiting { waker } => {
                eprintln!("Bad Cancel");
                abort();
            }
            _ => {}
        }
    }
}

impl<M> Debug for WaiterList<M> {
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

impl<M> Iterator for WaiterList<M> {
    type Item = *const Waiter<M>;

    fn next(&mut self) -> Option<Self::Item> {
        unsafe {
            if self.head != null() {
                let next = self.head.next();
                let old_head = mem::replace(&mut self.head, next);
                if self.head == null() {
                    self.tail = null();
                }
                Some(old_head)
            } else {
                None
            }
        }
    }
}

impl<M> Copy for FutexOwner<M> {}

impl<M> Clone for FutexOwner<M> {
    fn clone(&self) -> Self { *self }
}

impl<M> Packable for FutexOwner<M> {
    type Raw = usize2;
    unsafe fn encode(val: Self) -> Self::Raw { mem::transmute(val) }
    unsafe fn decode(val: Self::Raw) -> Self { mem::transmute(val) }
}

impl<M> Eq for FutexOwner<M> {}

impl<M> PartialEq for FutexOwner<M> {
    fn eq(&self, other: &Self) -> bool {
        self.first == other.first && self.second == other.second
    }
}

impl<M> Copy for WaiterList<M> {}

impl<M> Clone for WaiterList<M> {
    fn clone(&self) -> Self { *self }
}