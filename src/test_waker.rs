use std::task::{RawWakerVTable, Waker, RawWaker};
use crate::sync::atomic::AtomicUsize;
use crate::sync::atomic::Ordering::Relaxed;

pub struct TestWaker {
    refs: AtomicUsize,
    wakes: AtomicUsize,
    vtable: &'static RawWakerVTable,
}

pub const VTABLE: RawWakerVTable = RawWakerVTable::new(
    |x| unsafe { (x as *const TestWaker).waker_clone() },
    |x| unsafe { (x as *const TestWaker).waker_wake() },
    |x| unsafe { (x as *const TestWaker).waker_wake_by_ref() },
    |x| unsafe { (x as *const TestWaker).waker_drop() },
);

impl TestWaker {
    pub fn new() -> (&'static TestWaker, Waker) {
        Self::from_vtable(&VTABLE)
    }
    fn from_vtable(vtable: &'static RawWakerVTable) -> (&'static TestWaker, Waker) {
        let inner = Box::leak(Box::new(Self {
            refs: AtomicUsize::new(1),
            wakes: AtomicUsize::new(0),
            vtable,
        }));
        let waker = unsafe { Waker::from_raw(inner.raw()) };
        (inner, waker)
    }

    fn raw(&self) -> RawWaker {
        RawWaker::new(self as *const Self as *const (), self.vtable)
    }

    unsafe fn waker_clone(self: *const Self) -> RawWaker {
        assert!((*self).refs.fetch_add(1, Relaxed) > 0);
        RawWaker::new(self as *const (), (*self).vtable)
    }

    unsafe fn waker_wake(self: *const Self) {
        self.waker_wake_by_ref();
        self.waker_drop();
    }

    unsafe fn waker_wake_by_ref(self: *const Self) {
        assert!((*self).refs.load(Relaxed) > 0);
        (*self).wakes.fetch_add(1, Relaxed);
    }

    unsafe fn waker_drop(self: *const Self) {
        assert!((*self).refs.fetch_sub(1, Relaxed) > 0);
    }

    pub fn load(&self) -> (usize, usize) {
        (self.refs.load(Relaxed), self.wakes.load(Relaxed))
    }
}