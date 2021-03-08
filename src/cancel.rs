use crate::sync::atomic::AtomicBool;
use crate::sync::atomic::Ordering::Relaxed;
use crate::future::Future;
use std::task::{Context, Poll};
use std::pin::Pin;
use std::fmt::{Display, Formatter};
use std::fmt;

//TODO: this should be inside std::task::Context
pub struct Cancel {
    cancelling: AtomicBool,
    cancelled: AtomicBool,
}

impl Cancel {
    pub fn new() -> Self {
        Cancel {
            cancelling: Default::default(),
            cancelled: Default::default(),
        }
    }
    pub fn set_cancelling(&self) {
        self.cancelling.store(true, Relaxed);
    }
    pub fn cancelling(&self) -> bool {
        self.cancelling.load(Relaxed)
    }
    pub fn set_cancelled(&self) {
        self.cancelled.store(true, Relaxed)
    }
    pub fn cancelled(&self) -> bool {
        self.cancelled.load(Relaxed)
    }
}

struct WithCancel<'a, F: Future> {
    cancel: &'a Cancel,
    fut: F,
}

fn with_cancel<'a, F: Future>(cancel: &'a Cancel, fut: F) -> WithCancel<'a, F> {
    WithCancel { cancel: cancel, fut }
}

#[derive(Debug)]
struct Canceled;

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
            match Pin::new_unchecked(&mut this.fut).poll(cx) {
                Poll::Pending =>
                    if this.cancel.cancelled() {
                        Poll::Ready(Err(Canceled))
                    } else {
                        Poll::Pending
                    }
                Poll::Ready(result) =>
                    Poll::Ready(Ok(result)),
            }
        }
    }
}