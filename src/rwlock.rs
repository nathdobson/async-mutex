use std::backtrace::{Backtrace, BacktraceStatus};
use std::borrow::Borrow;
use std::collections::HashMap;
use std::lazy::SyncOnceCell;
use std::marker::PhantomData;
use std::mem;
use std::mem::{align_of, MaybeUninit, size_of};
use std::ops::{Deref, DerefMut};
use std::pin::Pin;
use std::process::abort;
use std::ptr::{null, null_mut};
use std::sync::Weak;
use std::task::{Context, Poll, RawWaker, RawWakerVTable};
use std::task::Waker;

use crate::futex::atomic::{Atomic, Packable};
use crate::futex::atomic_impl::{AtomicUsize2, usize2};
use crate::cell::UnsafeCell;
use crate::futex::{Futex, WaitFuture, WaitAction, Flow};
use crate::future::Future;
use crate::sync::Arc;
use crate::sync::atomic::AtomicBool;
use crate::sync::atomic::AtomicUsize;
use crate::sync::atomic::Ordering::{AcqRel, Acquire, Relaxed, Release};
use crate::test_println;
use crate::util::{AsyncFnOnce, Bind, FnOnceExt};
use crate::futex::waiter::Waiter;

#[derive(Debug)]
pub struct RwLock<T> {
    pub(crate) futex: Futex<()>,
    inner: UnsafeCell<T>,
}

#[derive(Debug)]
pub struct ReadGuard<'a, T> {
    pub(crate) mutex: &'a RwLock<T>,
}

#[derive(Debug)]
pub struct WriteGuard<'a, T> {
    pub(crate) mutex: &'a RwLock<T>,
}

#[derive(Copy, Clone, Eq, PartialEq, Debug)]
pub struct State {
    readers: usize,
    writing: bool,
}

impl<T> RwLock<T> {
    pub fn new(inner: T) -> Self {
        RwLock { futex: Futex::new(0), inner: UnsafeCell::new(inner) }
    }

    pub fn get_mut(&mut self) -> &mut T {
        unsafe { self.inner.with_mut(|x| &mut *x) }
    }

    pub async fn read<'a>(&'a self) -> ReadGuard<'a, T> {
        unsafe {
            self.futex.wait(
                (),
                |state: State| {
                    if state.writing {
                        WaitAction { update: None, flow: Flow::Pending(()) }
                    } else {
                        WaitAction {
                            update: Some(State { readers: state.readers + 1, writing: false }),
                            flow: Flow::Ready(()),
                        }
                    }
                },
                |_, _| (),
                |_, _| self.read_unlock(),
                |_, _| (),
            ).await;
            ReadGuard { mutex: self }
        }
    }

    pub(crate) unsafe fn read_unlock(&self) {
        if self.futex.update(|mut state: State| {
            if state == (State { readers: 1, writing: true }) {
                None
            } else {
                Some(State { readers: state.readers - 1, writing: state.writing })
            }
        }).is_ok() {
            return;
        }
        let state = self.futex.lock_state();
        let waiter = state.with_mut(|state| {
            self.futex.update_flip(&mut *state, |atom, queued| {

            });
            self.futex.pop(&mut *state)
        });
        assert!(flipped != waiter.is_some());
        if let Some(waiter) = waiter {
            test_println!("Waiter is done");
            waiter.done();
        } else {
            test_println!("Flipped off");
        }
    }

    pub(crate) unsafe fn write_unlock(&self) {

    }
}

const WRITING_MASK: usize = 1 << (usize::BITS - 1);
const READERS_MASK: usize = !WRITING_MASK;

impl Packable for State {
    type Raw = usize;

    unsafe fn encode(val: Self) -> Self::Raw {
        assert!(val.readers < usize::MAX >> 1);
        (if val.writing { WRITING_MASK } else { 0 }) | val.readers
    }

    unsafe fn decode(val: Self::Raw) -> Self {
        State { readers: val & READERS_MASK, writing: val & WRITING_MASK == WRITING_MASK }
    }
}

impl<'a, T> Drop for ReadGuard<'a, T> {
    fn drop(&mut self) {
        unsafe { self.mutex.read_unlock(); }
    }
}

impl<'a, T> Drop for WriteGuard<'a, T> {
    fn drop(&mut self) {
        unsafe { self.mutex.write_unlock(); }
    }
}

unsafe impl<T: Send> Send for RwLock<T> {}

unsafe impl<T: Send> Sync for RwLock<T> {}

unsafe impl<'a, T: Send + Sync> Send for ReadGuard<'a, T> {}

unsafe impl<'a, T: Send + Sync> Sync for ReadGuard<'a, T> {}

unsafe impl<'a, T: Send> Send for WriteGuard<'a, T> {}

unsafe impl<'a, T: Send + Sync> Sync for WriteGuard<'a, T> {}

impl<'a, T> Deref for ReadGuard<'a, T> {
    type Target = T;
    fn deref(&self) -> &Self::Target { unsafe { &*self.mutex.inner.with(|x| x) } }
}

impl<'a, T> Deref for WriteGuard<'a, T> {
    type Target = T;
    fn deref(&self) -> &Self::Target { unsafe { &*self.mutex.inner.with_mut(|x| x) } }
}

impl<'a, T> DerefMut for WriteGuard<'a, T> {
    fn deref_mut(&mut self) -> &mut Self::Target { unsafe { &mut *self.mutex.inner.with_mut(|x| x) } }
}

impl<T: Default> Default for RwLock<T> {
    fn default() -> Self {
        RwLock::new(T::default())
    }
}