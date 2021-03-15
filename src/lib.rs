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

#[cfg(test)]
extern crate test;

pub(crate) use crate::loom::*;

mod loom;
mod util;
#[cfg(test)]
mod tests;
pub mod fair_mutex;
//pub mod unfair_mutex;
pub mod futex;
pub mod spin_lock;
pub mod condvar;
pub mod mpsc;
pub mod traits;
//pub mod mpsc;

pub use fair_mutex::Mutex;
pub use fair_mutex::MutexGuard;