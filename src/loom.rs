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
pub use loom::future;

#[cfg(not(loom))]
pub use std::future;