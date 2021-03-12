use test::Bencher;
use futures::executor::ThreadPool;
use futures::task::{SpawnExt, Spawn};
use crate::Mutex;
use crate::loom::sync::Arc;
use std::mem;
use crate::util::yield_now;
use crate::future::block_on;
use futures::future::{join_all};
use std::ops::DerefMut;
use tokio::runtime::Runtime;

async fn step(mut guard: impl DerefMut<Target=usize>) {
    *guard += 1;
    mem::drop(guard);
}

trait IsMutex {
    fn new() -> Self;
    fn add(self: Arc<Self>, steps: usize) -> tokio::task::JoinHandle<()>;
}


fn bench_mutex<T: IsMutex>(b: &mut Bencher) {
    let tasks = 2usize;
    let count = 1000usize;
    let rt = Runtime::new().unwrap();
    let rte = rt.enter();
    b.iter(|| {
        let mutex = Arc::new(T::new());
        block_on(join_all((0..tasks).map(|task| mutex.clone().add(count))));
    });
}

impl IsMutex for Mutex<usize> {
    fn new() -> Self {
        Mutex::new(0)
    }
    fn add(self: Arc<Self>, steps: usize) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            for i in 0..steps {
                step(self.lock().await).await;
            }
        })
    }
}

#[bench]
fn bench_self(b: &mut Bencher) {
    bench_mutex::<Mutex<usize>>(b);
}

impl IsMutex for tokio::sync::Mutex<usize> {
    fn new() -> Self {
        tokio::sync::Mutex::new(0)
    }
    fn add(self: Arc<Self>, steps: usize) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            for i in 0..steps {
                step(self.lock().await).await;
            }
        })
    }
}

#[bench]
fn bench_tokio_sync_mutex(b: &mut Bencher) {
    bench_mutex::<tokio::sync::Mutex<usize>>(b);
}

impl IsMutex for async_std::sync::Mutex<usize> {
    fn new() -> Self {
        Self::new(0)
    }
    fn add(self: Arc<Self>, steps: usize) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            for i in 0..steps {
                step(self.lock().await).await;
            }
        })
    }
}

#[bench]
fn bench_async_std_sync_mutex(b: &mut Bencher) {
    bench_mutex::<async_std::sync::Mutex<usize>>(b);
}

impl IsMutex for futures::lock::Mutex<usize> {
    fn new() -> Self {
        Self::new(0)
    }
    fn add(self: Arc<Self>, steps: usize) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            for i in 0..steps {
                step(self.lock().await).await;
            }
        })
    }
}

#[bench]
fn bench_futures_lock_mutex(b: &mut Bencher) {
    bench_mutex::<futures::lock::Mutex<usize>>(b);
}

impl IsMutex for async_mutex::Mutex<usize> {
    fn new() -> Self {
        Self::new(0)
    }
    fn add(self: Arc<Self>, steps: usize) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            for i in 0..steps {
                step(self.lock().await).await;
            }
        })
    }
}

#[bench]
fn bench_async_mutex_mutex(b: &mut Bencher) {
    bench_mutex::<async_mutex::Mutex<usize>>(b);
}


impl IsMutex for futures_locks::Mutex<usize> {
    fn new() -> Self {
        Self::new(0)
    }
    fn add(self: Arc<Self>, steps: usize) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            for i in 0..steps {
                step(self.lock().await).await;
            }
        })
    }
}

#[bench]
fn bench_futures_locks_mutex(b: &mut Bencher) {
    bench_mutex::<async_mutex::Mutex<usize>>(b);
}