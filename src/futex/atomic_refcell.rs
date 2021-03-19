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
//
// #[cfg(test)]
// mod test {
//     use test::Bencher;
//     use crate::sync::atomic::AtomicBool;
//     use std::hint::black_box;
//     use crate::sync::atomic::Ordering::Release;
//     use crate::sync::atomic::Ordering::Acquire;
//     use crate::sync::atomic::Ordering::Relaxed;
//     use crate::futex::Atomic;
//
//     #[bench]
//     fn flip_bool(bencher: &mut Bencher) {
//         let x = Atomic::<usize>::new(0);
//         let mut foo = 0;
//         bencher.iter(|| {
//             let mut b: usize = 0;
//             if x.cmpxchg(&mut b, 1, Acquire, Relaxed) {
//                 foo = black_box(foo);
//                 x.store(1, Release);
//             }
//             foo = black_box(foo);
//         });
//         println!("{:?}", foo);
//     }
// }