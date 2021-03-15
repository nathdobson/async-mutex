#[cfg(loom)]
pub use loom::model;

#[cfg(not(loom))]
pub fn model(x: impl Fn() + Sync + Send + 'static) {
    panic!("Loom is disabled");
}

#[cfg(loom)]
pub mod sync {
    pub use loom::sync::*;

    pub mod atomic {
        pub use loom::sync::atomic::*;

        pub trait FetchMax {
            type Raw;
            fn fetch_max(&self, raw: Self::Raw, ordering: Ordering) -> Self::Raw;
        }

        impl FetchMax for AtomicIsize {
            type Raw = isize;

            fn fetch_max(&self, raw: isize, ordering: Ordering) -> isize {
                std::process::abort()
            }
        }
    }
}


#[cfg(not(loom))]
pub mod sync {
    pub use std::sync::*;

    pub mod atomic {
        pub use std::sync::atomic::*;

        pub trait FetchMax {}
    }
}

#[cfg(loom)]
pub mod thread {
    pub use loom::thread::*;

    pub fn park() { std::process::abort() }

    pub trait Unpark {
        fn unpark(&self) {}
    }

    impl Unpark for Thread {}
}

#[cfg(not(loom))]
pub mod thread {
    pub use std::thread::*;

    pub trait Unpark {}
}

#[cfg(loom)]
pub mod future {
    pub use loom::future::*;
    pub use std::future::Future;
    pub use std::future::poll_fn;
}

#[cfg(not(loom))]
pub mod future {
    pub use std::future::*;
    #[cfg(test)]
    pub use futures::executor::block_on;
}

#[cfg(loom)]
pub mod cell {
    pub use loom::cell::*;
}

#[cfg(not(loom))]
pub mod cell {
    #[derive(Debug)]
    pub struct UnsafeCell<T>(std::cell::UnsafeCell<T>);

    impl<T> UnsafeCell<T> {
        pub fn new(x: T) -> Self {
            UnsafeCell(std::cell::UnsafeCell::new(x))
        }
        pub unsafe fn with<F, R>(&self, f: F) -> R where F: FnOnce(*const T) -> R {
            f(self.0.get())
        }
        pub unsafe fn with_mut<F, R>(&self, f: F) -> R where F: FnOnce(*mut T) -> R {
            f(self.0.get())
        }
        pub fn into_inner(self) -> T {
            self.0.into_inner()
        }
        #[cfg(not(loom))]
        pub unsafe fn get_mut(&mut self) -> &mut T {
            self.0.get_mut()
        }
    }
}