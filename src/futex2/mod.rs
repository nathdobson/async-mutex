pub mod mutex;

use crate::atomic::{CopyWaker, Atomic, Packable, PANIC_WAKER_VTABLE, usize2, usize_half};
use crate::sync::Mutex;
use std::task::{Waker, Poll, Context, RawWakerVTable};
use crate::sync::atomic::Ordering::Relaxed;
use crate::sync::atomic::Ordering::Acquire;
use crate::sync::atomic::Ordering::Release;
use crate::sync::atomic::Ordering::AcqRel;
use crate::sync::atomic::AtomicUsize;
use std::ops::Index;
use crate::future::Future;
use std::pin::Pin;
use std::mem;
use std::ptr::null;
use crate::thread;

#[derive(Debug, Copy, Clone)]
enum Bucket {
    Empty { iteration: usize_half },
    Woken { iteration: usize_half },
    Waiting { waker: CopyWaker },
    Waking,
    Tombstone,
}

#[derive(Debug)]
struct Generation {
    waiters: Vec<Atomic<Bucket>>,
}

#[derive(Debug, Copy, Clone)]
struct Atom {
    generation: *const Generation,
    waits: usize_half,
    notifies: usize_half,
}

#[derive(Debug)]
pub struct Queue {
    atom: Atomic<Atom>,
    generations: Mutex<Vec<*const Generation>>,
}

impl Index<usize_half> for Generation {
    type Output = Atomic<Bucket>;

    fn index(&self, index: usize_half) -> &Self::Output {
        &self.waiters[(index as usize) % self.waiters.len()]
    }
}

enum WaitFutureStep<F: FnOnce() -> bool> {
    Entering { condition: F },
    Waiting { generation: *const Generation, iteration: usize_half },
    Done,
}

pub struct WaitFuture<'a, F: FnOnce() -> bool> {
    queue: &'a Queue,
    step: WaitFutureStep<F>,
}

impl Generation {
    fn new(cap: usize) -> Self {
        assert!(cap <= (usize_half::MAX as usize) + 1);
        Generation {
            waiters: (0..cap).map(|iteration|
                Atomic::new(Bucket::Empty { iteration: iteration as usize_half }))
                .collect()
        }
    }
}

impl Queue {
    pub fn with_capacity(cap: usize) -> Self {
        let generation = Box::into_raw(Box::new(Generation::new(cap)));
        test_println!("Initial {:?}", generation);
        Queue {
            atom: Atomic::new(Atom { generation, waits: 0, notifies: 0 }),
            generations: Mutex::new(vec![generation]),
        }
    }

    unsafe fn expand(&self, generation: *const Generation) {
        let mut lock = self.generations.lock().unwrap();
        if &**lock.last().unwrap() as *const Generation == generation {
            let new_capacity = (*generation).waiters.len() * 2;
            for x in (*generation).waiters.iter() {
                match x.swap(Bucket::Tombstone, AcqRel) {
                    Bucket::Empty { .. } => test_println!("empty -> tombstone"),
                    Bucket::Woken { .. } => test_println!("woken -> tombstone"),
                    Bucket::Waiting { waker } => {
                        test_println!("Waking from old generation: {:?}",waker);
                        waker.into_waker().wake()
                    }
                    Bucket::Waking => test_println!("waking -> tombstone"),
                    Bucket::Tombstone => {}
                }
            }
            let generation = Box::into_raw(Box::new(Generation::new(new_capacity)));
            test_println!("Expanding {:?}", generation);
            self.atom.swap(Atom { generation, waits: 0, notifies: 0 }, AcqRel);
            test_println!("Done expanding");
            lock.push(generation);
        } else {
            test_println!("No expand");
        }
    }

    pub fn wait_if<F: FnOnce() -> bool>(&self, condition: F) -> WaitFuture<F> {
        WaitFuture { queue: self, step: WaitFutureStep::Entering { condition } }
    }

    pub fn notify(&self, add: usize) {
        let add = add as usize_half;
        unsafe {
            let mut atom = self.atom.load(Acquire);
            let mut new_notifies;
            loop {
                new_notifies = (atom.notifies + add).min(atom.waits);
                let new_atom = Atom { notifies: new_notifies, ..atom };
                if self.atom.cmpxchg_weak(&mut atom, new_atom, AcqRel, Acquire) {
                    break;
                }
            }
            let generation = atom.generation;
            let old_notifies = atom.notifies;
            test_println!("Notifying {:?} {:?} {:?} -> {:?}", thread::current().id(), generation,old_notifies, new_notifies);
            for iteration in old_notifies..new_notifies {
                let bucket = &(*generation)[iteration];
                let mut old = bucket.load(Acquire);
                loop {
                    match old {
                        Bucket::Waiting { waker } => {
                            if bucket.cmpxchg_weak(
                                &mut old, Bucket::Waking,
                                AcqRel, Acquire,
                            ) {
                                test_println!("Waking {:?} {:?}", iteration, waker);
                                waker.into_waker().wake();
                                break;
                            }
                        }
                        Bucket::Empty { iteration: old_iteration } if iteration == old_iteration => {
                            if bucket.cmpxchg_weak(
                                &mut old, Bucket::Woken { iteration },
                                AcqRel, Acquire,
                            ) {
                                break;
                            }
                        }
                        Bucket::Waking | Bucket::Woken { .. } | Bucket::Empty { .. } | Bucket::Tombstone => {
                            self.expand(generation);
                            return;
                        }
                    }
                }
            }
        }
    }
}

impl<'a, F: FnOnce() -> bool> WaitFuture<'a, F> {
    unsafe fn poll_enter(&mut self, cx: &mut Context, condition: F) -> Poll<()> {
        let mut atom = self.queue.atom.load(Acquire);
        let iteration;
        loop {
            let new_atom = Atom { waits: atom.waits + 1, ..atom };
            if self.queue.atom.cmpxchg_weak(&mut atom, new_atom, AcqRel, Acquire) {
                iteration = atom.waits;
                break;
            }
        }
        let generation = atom.generation;
        let cap = (*generation).waiters.len() as usize_half;
        let waker = CopyWaker::from_waker(cx.waker().clone());
        let bucket = &(*generation)[iteration];
        let mut current = Bucket::Empty { iteration };
        loop {
            match current {
                Bucket::Empty { iteration: iteration2 } if iteration == iteration2 => {
                    if bucket.cmpxchg_weak(
                        &mut current, Bucket::Waiting { waker },
                        AcqRel, Acquire) {
                        if condition() {
                            test_println!("Entered waiting state {:?} {:?} {:?} {:?}", thread::current().id(), generation, iteration, waker);
                            self.step = WaitFutureStep::Waiting { generation, iteration };
                            return Poll::Pending;
                        } else {
                            test_println!("Condition failed");
                            self.queue.notify(1);
                            return Poll::Ready(());
                        }
                    }
                }
                Bucket::Woken { iteration: iteration2 } if iteration2 == iteration => {
                    if bucket.cmpxchg_weak(
                        &mut current, Bucket::Empty { iteration: iteration + cap },
                        AcqRel, Acquire,
                    ) {
                        mem::drop(waker.into_waker());
                        self.step = WaitFutureStep::Done;
                        test_println!("Wake already complete");
                        return Poll::Ready(());
                    }
                }
                Bucket::Waiting { .. } | Bucket::Waking | Bucket::Empty { .. } | Bucket::Woken { .. } | Bucket::Tombstone => {
                    test_println!("Wait caused expand");
                    mem::drop(waker.into_waker());
                    self.queue.expand(generation);
                    return Poll::Ready(());
                }
            }
        }
    }
    unsafe fn poll_waiting(&mut self, cx: &mut Context, generation: *const Generation, iteration: usize_half) -> Poll<()> {
        let waker = CopyWaker::from_waker(cx.waker().clone());
        let bucket = &(*generation)[iteration];
        let cap = (*generation).waiters.len() as usize_half;
        let mut current = bucket.load(Acquire);
        loop {
            match current {
                Bucket::Waiting { waker: old_waker } => {
                    if bucket.cmpxchg_weak(
                        &mut current, Bucket::Waiting { waker },
                        AcqRel, Acquire,
                    ) {
                        mem::drop(old_waker.into_waker());
                        self.step = WaitFutureStep::Waiting { generation, iteration };
                        return Poll::Pending;
                    }
                }
                Bucket::Waking => {
                    if bucket.cmpxchg_weak(
                        &mut current, Bucket::Empty { iteration: iteration + cap },
                        AcqRel, Acquire,
                    ) {
                        mem::drop(waker.into_waker());
                        return Poll::Ready(());
                    }
                }
                Bucket::Tombstone => {
                    self.queue.expand(generation);
                    mem::drop(waker.into_waker());
                    return Poll::Ready(());
                }
                _ => unreachable!(),
            }
        }
    }
}

impl<'a, F: FnOnce() -> bool> Future for WaitFuture<'a, F> {
    type Output = ();
    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        unsafe {
            match mem::replace(&mut self.step, WaitFutureStep::Done) {
                WaitFutureStep::Entering { condition } => self.poll_enter(cx, condition),
                WaitFutureStep::Waiting { generation, iteration } =>
                    self.poll_waiting(cx, generation, iteration),
                WaitFutureStep::Done => panic!(),
            }
        }
    }
}

impl<'a, F: FnOnce() -> bool> Drop for WaitFuture<'a, F> {
    fn drop(&mut self) {
        match self.step {
            WaitFutureStep::Entering { .. } => {}
            WaitFutureStep::Waiting { .. } => {
                todo!();
            }
            WaitFutureStep::Done => {}
        }
    }
}

impl<'a, F: FnOnce() -> bool> Unpin for WaitFuture<'a, F> {}

static WOKEN: RawWakerVTable = PANIC_WAKER_VTABLE;
static WAKING: RawWakerVTable = PANIC_WAKER_VTABLE;
static EMPTY: RawWakerVTable = PANIC_WAKER_VTABLE;
static TOMBSTONE: RawWakerVTable = PANIC_WAKER_VTABLE;

impl Packable for Bucket {
    type Raw = usize2;

    unsafe fn encode(val: Self) -> Self::Raw {
        mem::transmute(match val {
            Bucket::Waiting { waker } => waker,
            Bucket::Woken { iteration } => CopyWaker(iteration as *const (), &WOKEN),
            Bucket::Waking => CopyWaker(null(), &WAKING),
            Bucket::Empty { iteration } => CopyWaker(iteration as *const (), &EMPTY),
            Bucket::Tombstone => CopyWaker(null(), &TOMBSTONE),
        })
    }

    unsafe fn decode(val: Self::Raw) -> Self {
        let waker: CopyWaker = mem::transmute(val);
        if waker.1 == &WOKEN {
            Bucket::Woken { iteration: waker.0 as usize_half }
        } else if waker.1 == &WAKING {
            Bucket::Waking
        } else if waker.1 == &EMPTY {
            Bucket::Empty { iteration: waker.0 as usize_half }
        } else if waker.1 == &TOMBSTONE {
            Bucket::Tombstone
        } else {
            Bucket::Waiting { waker }
        }
    }
}

impl Drop for Generation {
    fn drop(&mut self) {
        unsafe {
            for waiter in self.waiters.iter_mut() {
                match waiter.load_mut() {
                    Bucket::Waiting { waker } => mem::drop(waker.into_waker()),
                    Bucket::Woken { .. } => {}
                    Bucket::Waking => {}
                    Bucket::Empty { .. } => {}
                    Bucket::Tombstone => {}
                }
            }
        }
    }
}

unsafe impl Send for Queue {}

unsafe impl Sync for Queue {}

unsafe impl<'a, T: Send + FnOnce() -> bool> Send for WaitFuture<'a, T> {}

impl Packable for Atom {
    type Raw = usize2;

    unsafe fn encode(val: Self) -> Self::Raw {
        mem::transmute(val)
    }

    unsafe fn decode(val: Self::Raw) -> Self {
        mem::transmute(val)
    }
}