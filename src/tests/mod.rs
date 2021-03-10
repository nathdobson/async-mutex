#[cfg(loom)]
mod loom;
#[cfg(not(loom))]
mod noloom;
pub mod test;
#[cfg(not(loom))]
pub mod bench;

use futures::executor::{LocalPool, block_on};
use crate::test_waker::TestWaker;
use crate::{Mutex, MutexGuard, MutexScope, thread, util};
use std::task::{Context, Poll};
use crate::util::{FnOnceExt};
use futures::pin_mut;
use futures::Future;
use futures::future::ready;
use std::mem;
use futures::executor::ThreadPool;
use crate::sync::Arc;
use futures::task::{SpawnExt, Spawn};
use rand::thread_rng;
use futures::future::join_all;
use rand::seq::SliceRandom;
use std::path::Path;
use std::fs::read_dir;
use std::ffi::OsStr;
use crate::cancel::{Cancel, Canceled};
use crate::util::yield_now;
use futures::task::noop_waker_ref;

#[cfg(not(loom))]
#[test]
fn test_uses_loom() {
    let mut bad = false;
    fn test_directory(x: &Path, bad: &mut bool) {
        for file in read_dir(x).unwrap() {
            let file = file.unwrap();
            let path = file.path();
            if file.file_type().unwrap().is_dir() {
                test_directory(&path, bad);
            } else if path.extension() == Some(OsStr::new("rs")) {
                let contents = std::fs::read_to_string(&path).unwrap();
                for (line_number, line) in contents.split("\n").enumerate() {
                    if line.starts_with("use std::sync::atomic")
                        || line.starts_with("use std::thread")
                        || line.starts_with("use std::future")
                        || line.starts_with("use std::cell") {
                        eprintln!("{}:{} {}", path.as_os_str().to_str().unwrap(), line_number + 1, line);
                        *bad = true;
                    }
                }
            }
        }
    }
    test_directory(Path::new("src"), &mut bad);
    if bad {
        panic!("Bad uses");
    }
}

fn send_test() {
    async fn imp() {
        let cancel = Cancel::new();
        let mutex = Mutex::new(0usize);
        let mut x = 1usize;
        let scope = mutex.scope();
        scope.with(async {
            let guard = scope.lock(&cancel).await;
            yield_now().await;
            mem::drop(guard);
        }).await;
    }

    fn assert_send<X: Send>(x: X) {}

    fn assert_test2_send() {
        assert_send(imp())
    }
}

async fn now_or_cancel<F: Future>(cancel: &Cancel, fut: F) -> Result<F::Output, Canceled> {
    pin_mut!(fut);
    match fut.as_mut().poll(&mut Context::from_waker(noop_waker_ref())) {
        Poll::Ready(x) => return Ok(x),
        Poll::Pending => {}
    }
    yield_now().await;
    cancel.set_cancelling();
    cancel.run(fut).await
}