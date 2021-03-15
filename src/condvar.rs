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
    futex: Futex<()>,
}

impl Condvar {
    pub fn new() -> Self {
        Condvar { futex: Futex::new(0) }
    }
    pub async fn wait<'g, T>(&self, guard: MutexGuard<'g, T>) -> MutexGuard<'g, T> {
        unsafe {
            let mutex = guard.mutex;
            mem::forget(guard);
            let _: (Flow<!, _>, _) = self.futex.wait(
                (),
                |old| {
                    assert!(old == null() || old == mutex as *const Mutex<T>);
                    WaitAction { update: Some(mutex as *const Mutex<T>), flow: Flow::Pending(()) }
                },
                |_, _| mutex.unlock(),
                |_, _| mutex.unlock(),
                |_, _| todo!(),
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
            let lock = self.futex.lock_state();
            let waiters = lock.with_mut(|state| {
                self.futex.update_flip(&mut *state, |_: usize, _| None);
                self.futex.pop_many(&mut *state, count)
            });
            test_println!("Requeueing {:?}", waiters);
            guard.mutex.futex.requeue(&self.futex, waiters);
        }
    }
    pub fn notify_all<T>(&self, guard: &mut MutexGuard<T>) {
        self.notify(guard, usize::MAX);
    }
}
