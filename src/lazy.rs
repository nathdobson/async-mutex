use crate::futex::Futex;
use crate::atomic::{Packable, AcquireT, RelaxedT, IsAcquireT, AcqRelT, ReleaseT};
use std::mem;
use crate::cell::UnsafeCell;
use std::mem::MaybeUninit;
use crate::future::Future;
use crate::sync::atomic::Ordering::Relaxed;
use crate::thread::panicking;
use pin_utils::pin_mut;

#[derive(Debug, Copy, Clone, Eq, PartialEq)]
#[repr(usize)]
enum CellAtom {
    Uninit,
    Active,
    Poison,
    Init,
}

pub struct SyncOnceCell<T> {
    futex: Futex<CellAtom>,
    inner: UnsafeCell<MaybeUninit<T>>,
}

impl<T> SyncOnceCell<T> {
    pub fn new() -> Self {
        SyncOnceCell { futex: Futex::new(CellAtom::Uninit, 1), inner: UnsafeCell::new(MaybeUninit::uninit()) }
    }

    pub fn get(&self) -> Option<&T> {
        match self.futex.load(AcquireT).inner() {
            CellAtom::Uninit => None,
            CellAtom::Active => None,
            CellAtom::Poison => panic!("Panic occurred while initializing"),
            CellAtom::Init => Some(unsafe { self.assume_init_ref() }),
        }
    }

    unsafe fn assume_init_ref(&self) -> &T {
        (*self.inner.with(|x| x)).assume_init_ref()
    }

    fn finish(&self, new: CellAtom) {
        let waiters = self.futex.lock();
        let mut queue = waiters.lock();
        let mut atom = self.futex.load(RelaxedT);
        loop {
            assert_eq!(atom.inner(), CellAtom::Active);
            if queue.cmpxchg_enqueue_weak(&mut atom, new, AcqRelT, RelaxedT) {
                break;
            }
        }
        for waiter in queue.pop_many(usize::MAX, 0) {
            waiter.wake()
        }
    }

    pub async fn get_or_init(&self, init: impl Future<Output=T>) -> &T {
        let mut atom = self.futex.load(AcquireT);
        let waiter = self.futex.waiter(0, 0);
        pin_mut!(waiter);
        loop {
            match atom.inner() {
                CellAtom::Uninit => {
                    if self.futex.cmpxchg_weak(&mut atom, CellAtom::Active, RelaxedT, AcquireT) {
                        struct OnDrop<'a, T>(&'a SyncOnceCell<T>);
                        impl<'a, T> Drop for OnDrop<'a, T> {
                            fn drop(&mut self) {
                                self.0.finish(CellAtom::Poison);
                            }
                        }
                        let on_drop = OnDrop(self);
                        let value = init.await;
                        unsafe { (*self.inner.with_mut(|x| x)).write(value) };
                        mem::forget(on_drop);
                        self.finish(CellAtom::Init);
                        return unsafe { self.assume_init_ref() };
                    }
                }
                CellAtom::Active => {
                    if self.futex.cmpxchg_wait_weak(
                        waiter.as_mut(),
                        &mut atom,
                        CellAtom::Active,
                        ReleaseT,
                        AcquireT,
                        || {},
                        || {},
                        || {},
                    ).await {
                        // TODO: make RelaxedT?
                        atom = self.futex.load(AcquireT);
                    }
                }
                CellAtom::Poison => panic!("Panic occurred while initializing"),
                CellAtom::Init => return unsafe { self.assume_init_ref() },
            }
        }
    }
}

impl Packable for CellAtom {
    type Raw = usize;
    unsafe fn encode(val: Self) -> Self::Raw { mem::transmute(val) }
    unsafe fn decode(val: Self::Raw) -> Self { mem::transmute(val) }
}

unsafe impl<T: Send> Send for SyncOnceCell<T> {}

unsafe impl<T: Sync + Send> Sync for SyncOnceCell<T> {}