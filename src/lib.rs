#![allow(unused_imports)]
#![allow(unused_variables)]
#![allow(unused_mut)]
#![allow(dead_code)]
#![allow(unused_unsafe)]
#![deny(unused_must_use)]

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

#[cfg(test)]
extern crate test;

pub(crate) use crate::loom::*;

mod atomic;
mod loom;
mod util;
mod atomic_impl;
#[cfg(test)]
mod tests;
mod state;
mod waiter;
mod mutex;
//mod condvar;
pub mod futex;

pub use mutex::Mutex;
pub use mutex::MutexGuard;
