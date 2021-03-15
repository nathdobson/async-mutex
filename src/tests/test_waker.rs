use std::task::{RawWakerVTable, Waker, RawWaker, Poll, Context};
use crate::sync::atomic::AtomicUsize;
use crate::sync::atomic::Ordering::Relaxed;
use futures::future::BoxFuture;
use std::pin::Pin;
use crate::future::Future;
use futures::FutureExt;

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

    pub fn refs(&self) -> usize {
        self.refs.load(Relaxed)
    }
    pub fn clear_wakes(&self) -> usize {
        self.wakes.swap(0, Relaxed)
    }
}

struct TestTaskInner {
    test_waker: &'static TestWaker,
    waker: Waker,
    awake: bool,
}

pub struct TestTask<F> {
    index: usize,
    fut: Pin<Box<F>>,
}

pub struct TestPool {
    tasks: Vec<TestTaskInner>,
}

impl TestPool {
    pub fn new() -> Self {
        TestPool{ tasks: vec![] }
    }
    pub fn spawn<F: Future>(&mut self, fut: F) -> TestTask<F> {
        let (test_waker, waker) = TestWaker::new();
        let index = self.tasks.len();
        self.tasks.push(TestTaskInner {
            test_waker,
            waker,
            awake: true,
        });
        TestTask { index, fut: Box::pin(fut) }
    }
    pub fn step<F: Future>(&mut self, future: &mut TestTask<F>) -> (Poll<F::Output>, Vec<usize>) {
        assert!(self.tasks[future.index].awake);
        self.tasks[future.index].awake = false;
        let mut cx = Context::from_waker(&self.tasks[future.index].waker);
        let result = future.fut.as_mut().poll(&mut cx);
        let mut wakes = vec![];
        for (index, task) in self.tasks.iter_mut().enumerate() {
            let wakeups = task.test_waker.clear_wakes();
            if wakeups > 0 {
                task.awake = true;
            }
            for _ in 0..wakeups {
                wakes.push(index);
            }
            assert!(task.awake || task.test_waker.refs() > 0);
        }
        (result, wakes)
    }
}