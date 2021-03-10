use crate::sync::atomic::AtomicBool;
use crate::sync::atomic::Ordering::Relaxed;
use crate::future::{Future};
use std::task::{Context, Poll};
use std::pin::Pin;
use std::fmt::{Display, Formatter};
use std::fmt;

//TODO: this should be inside std::task::Context
pub struct Cancel {
    cancelling: AtomicBool,
    pending: AtomicBool,
}

impl Cancel {
    pub fn new() -> Self {
        Cancel {
            cancelling: Default::default(),
            pending: Default::default(),
        }
    }
    pub fn set_cancelling(&self) {
        self.cancelling.store(true, Relaxed);
    }
    pub fn cancelling(&self) -> bool {
        self.cancelling.load(Relaxed)
    }
    pub fn clear_pending(&self) {
        self.pending.store(false, Relaxed)
    }
    pub fn set_pending(&self) {
        self.pending.store(true, Relaxed)
    }
    pub fn pending(&self) -> bool {
        self.pending.load(Relaxed)
    }
    pub fn run<'a, F: Future>(&'a self, fut: F) -> WithCancel<'a, F> {
        WithCancel { cancel: self, fut }
    }
}

pub struct WithCancel<'a, F: Future> {
    cancel: &'a Cancel,
    fut: F,
}

#[derive(Debug)]
pub struct Canceled;

impl Display for Canceled {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "Future was canceled")
    }
}

impl<'a, F: Future> Future for WithCancel<'a, F> {
    type Output = Result<F::Output, Canceled>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        unsafe {
            let this = self.get_unchecked_mut();
            this.cancel.clear_pending();
            match Pin::new_unchecked(&mut this.fut).poll(cx) {
                Poll::Pending =>
                    if this.cancel.pending() {
                        Poll::Pending
                    } else {
                        Poll::Ready(Err(Canceled))
                    }
                Poll::Ready(result) =>
                    Poll::Ready(Ok(result)),
            }
        }
    }
}
