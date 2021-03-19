use crate::futex::{Waiter, WaiterList};
use crate::futex::Atomic;
use std::ptr::null;
use std::mem::MaybeUninit;
use std::mem;
use crate::cell::UnsafeCell;
use crate::future::Future;
use std::task::{Context, Poll};
use std::pin::Pin;
use crate::sync::atomic::Ordering::Acquire;
use crate::sync::atomic::Ordering::AcqRel;
use crate::futex::{Futex, WaitFuture, Flow, WaitAction};
use crate::fair_mutex::MutexGuard;
use crate::fair_mutex::Mutex;
use std::marker::PhantomData;
use crate::sync::atomic::Ordering::Relaxed;
//use crate::test_println;

pub struct Condvar {
    futex: Futex,
}

impl Condvar {
    pub fn new() -> Self {
        Condvar { futex: Futex::new(0usize, 1) }
    }
    pub async fn wait<'g, T>(&self, guard: MutexGuard<'g, T>) -> MutexGuard<'g, T> {
        unsafe {
            let mutex = guard.mutex;
            mem::forget(guard);
            let _: Flow<!, _> = self.futex.wait(
                0,
                0,
                |old| {
                    assert!(old == null() || old == mutex as *const Mutex<T>);
                    WaitAction { update: Some(mutex as *const Mutex<T>), flow: Flow::Pending(()) }
                },
                |_| mutex.unlock(),
                |_| mutex.unlock(),
                |_| todo!(),
            ).await;
            MutexGuard { mutex }
        }
    }
    pub fn notify<T>(&self, guard: &mut MutexGuard<T>, count: usize) {
        unsafe {
            let mut atom = self.futex.load(Acquire);
            let mutex = atom.inner::<*const Mutex<T>>();
            if mutex == null() {
                test_println!("Unused condvar");
                return;
            }
            if guard.mutex as *const Mutex<T> != mutex {
                panic!("Different mutex");
            }
            let mut waiters = self.futex.lock();
            let mut queue = waiters.lock();
            while !queue.cmpxchg_enqueue_weak(&mut atom, mutex, Acquire, Relaxed) {}
            let list = queue.pop_many(count, 0);
            test_println!("Requeueing {:?}", list);
            mem::drop(queue);
            waiters.requeue(&guard.mutex.futex, list);
        }
    }
    pub fn notify_all<T>(&self, guard: &mut MutexGuard<T>) {
        self.notify(guard, usize::MAX);
    }
}
