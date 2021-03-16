use crate::futex::waiter::{Waiter, WaiterList};
use crate::futex::atomic::Atomic;
use std::ptr::null;
use std::mem::MaybeUninit;
use std::mem;
use crate::cell::UnsafeCell;
use crate::future::Future;
use std::task::{Context, Poll};
use std::pin::Pin;
use crate::futex::state::{CopyWaker, WaiterWaker};
use crate::sync::atomic::Ordering::Acquire;
use crate::sync::atomic::Ordering::AcqRel;
use crate::futex::{Futex, WaitFuture, RequeueResult, Flow, WaitAction};
use crate::fair_mutex::MutexGuard;
use crate::fair_mutex::Mutex;
use std::marker::PhantomData;
use crate::sync::atomic::Ordering::Relaxed;
use crate::test_println;

pub struct Condvar {
    futex: Futex,
}

impl Condvar {
    pub fn new() -> Self {
        Condvar { futex: Futex::new(0, 1) }
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
            let mutex: *const Mutex<T> = self.futex.load(Acquire);
            if mutex == null() {
                test_println!("Unused condvar");
                return;
            }
            if guard.mutex as *const Mutex<T> != mutex {
                panic!("Different mutex");
            }
            let lock = self.futex.lock();
            lock.update_flip(|_: usize, _| None);
            let waiters = lock.pop_many(count, 0);
            test_println!("Requeueing {:?}", waiters);
            lock.requeue(&guard.mutex.futex, waiters);
        }
    }
    pub fn notify_all<T>(&self, guard: &mut MutexGuard<T>) {
        self.notify(guard, usize::MAX);
    }
}
