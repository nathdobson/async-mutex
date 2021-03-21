use crate::sync::atomic::AtomicUsize;
use crate::cell::UnsafeCell;
use crate::sync::atomic::Ordering::Relaxed;
use crate::sync::atomic::Ordering::Acquire;
use crate::sync::atomic::Ordering::Release;
use std::ops::{Deref, DerefMut};

#[derive(Debug)]
pub struct AtomicRefCell<T> {
    inner: UnsafeCell<T>,
    state: AtomicUsize,
}

pub struct RefMut<'a, T> {
    cell: &'a AtomicRefCell<T>,
}

impl<T> AtomicRefCell<T> {
    pub fn new(inner: T) -> Self {
        Self { state: AtomicUsize::new(0), inner: UnsafeCell::new(inner) }
    }
    pub fn get_mut(&mut self) -> &mut T {
        unsafe { &mut *self.inner.with_mut(|x| x) }
    }
    pub fn borrow_mut(&self) -> RefMut<T> {
        assert!(self.state.compare_exchange(0, 1, Acquire, Relaxed).is_ok());
        RefMut { cell: self }
    }
}

impl<'a, T> Drop for RefMut<'a, T> {
    fn drop(&mut self) {
        self.cell.state.store(0, Release);
    }
}

impl<'a, T> Deref for RefMut<'a, T> {
    type Target = T;
    fn deref(&self) -> &Self::Target {
        unsafe { &*self.cell.inner.with_mut(|x| x) }
    }
}

impl<'a, T> DerefMut for RefMut<'a, T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        unsafe { &mut *self.cell.inner.with_mut(|x| x) }
    }
}