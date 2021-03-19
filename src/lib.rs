#![allow(unused_imports)]
#![allow(unused_variables)]
#![allow(unused_mut)]
#![allow(dead_code)]
#![allow(unused_unsafe)]
#![deny(unused_must_use)]

//#![feature(const_fn, const_generics)]
#![feature(arbitrary_self_types)]
#![feature(unboxed_closures)]
#![feature(once_cell)]
#![feature(fn_traits)]
#![feature(negative_impls)]
#![feature(integer_atomics)]
#![feature(cfg_target_has_atomic)]
#![feature(backtrace)]
#![feature(stmt_expr_attributes)]
#![feature(raw_ref_op)]
#![feature(future_poll_fn)]
#![feature(test)]
#![feature(bool_to_option)]
#![feature(associated_type_bounds)]
#![feature(maybe_uninit_extra)]
#![feature(maybe_uninit_ref)]
#![feature(never_type)]
#![feature(default_free_fn)]
#![feature(int_bits_const)]
#![feature(new_uninit)]

#[cfg(test)]
extern crate test;


#[macro_use]
mod async_traits;
#[macro_use]
mod util;

pub(crate) use crate::loom::*;

mod loom;
#[cfg(test)]
mod tests;
mod fair_mutex;
pub mod futex;
mod spin_lock;
mod condvar;
mod mpsc;
mod rwlock;
pub mod fast_mutex;

pub use fair_mutex::Mutex;
pub use fair_mutex::MutexGuard;
pub use condvar::Condvar;
pub use rwlock::RwLock;
pub use rwlock::ReadGuard;
pub use rwlock::WriteGuard;
pub use mpsc::channel;
pub use mpsc::Receiver;
pub use mpsc::Sender;


