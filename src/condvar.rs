use crate::futex::{Waiter};
use std::ptr::null;
use std::mem::MaybeUninit;
use std::mem;
use crate::cell::UnsafeCell;
use crate::future::Future;
use std::task::{Context, Poll};
use std::pin::Pin;
use crate::sync::atomic::Ordering::Acquire;
use crate::sync::atomic::Ordering::AcqRel;
use crate::futex::{Futex};
use crate::fair_mutex::MutexGuard;
use crate::fair_mutex::Mutex;
use std::marker::PhantomData;
use crate::sync::atomic::Ordering::Relaxed;
//use crate::test_println;
use pin_utils::pin_mut;
use crate::atomic::{RelaxedT, AcquireT, ReleaseT};

pub struct Condvar {
    futex: Futex<usize>,
}

impl Condvar {
    pub fn new() -> Self {
        Condvar { futex: Futex::new(0usize, 1) }
    }
    pub async fn wait<'g, T>(&self, guard: MutexGuard<'g, T>) -> MutexGuard<'g, T> {
        let mutex = guard.mutex;
        let mutex_ptr = mutex as *const Mutex<T> as usize;
        mem::forget(guard);
        let waiter = self.futex.waiter(0, 0);
        pin_mut!(waiter);
        let mut futex_atom = self.futex.load(Relaxed);
        loop {
            let atom = futex_atom.inner();
            assert!(atom == 0 || atom == mutex_ptr);
            if self.futex.cmpxchg_wait_weak(
                waiter.as_mut(),
                &mut futex_atom,
                mutex_ptr,
                ReleaseT,
                RelaxedT,
                || unsafe { mutex.unlock() },
                || unsafe { mutex.unlock() },
                || todo!(),
            ).await {
                break;
            }
        }
        MutexGuard { mutex }
    }
    pub fn notify<T>(&self, guard: &mut MutexGuard<T>, count: usize) {
        let mut atom = self.futex.load(Acquire);
        let mutex = atom.inner();
        if mutex == 0 {
            test_println!("Unused condvar");
            return;
        }
        if guard.mutex as *const Mutex<T> as usize != mutex {
            panic!("Different mutex");
        }
        let mut waiters = self.futex.lock();
        let mut queue = waiters.lock();
        while !queue.cmpxchg_enqueue_weak(&mut atom, mutex as *const Mutex<T> as usize, AcquireT, RelaxedT) {}
        let list = queue.pop_many(count, 0);
        test_println!("Requeueing {:?}", list);
        mem::drop(queue);
        guard.mutex.futex.requeue( list);
    }
    pub fn notify_all<T>(&self, guard: &mut MutexGuard<T>) {
        self.notify(guard, usize::MAX);
    }
}
