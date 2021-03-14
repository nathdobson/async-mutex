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
use crate::futex::{Futex, WaitFuture, WaitImpl, EnterResult, RequeueResult};
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
        Condvar { futex: Futex::new(0) }
    }
    pub async fn wait<'g, T>(&self, guard: MutexGuard<'g, T>) -> MutexGuard<'g, T> {
        unsafe {
            let mutex = guard.mutex;
            mem::forget(guard);
            self.futex.wait(WaitImpl {
                enter: |old| {
                    assert!(old == 0 || old == mutex as *const Mutex<T> as usize);
                    EnterResult { userdata: Some(mutex as *const Mutex<T> as usize), pending: true }
                },
                post_enter: Some(|| {
                    mutex.unlock();
                }),
                exit: || MutexGuard { mutex },
                phantom: PhantomData::<usize>,
            }).await
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
