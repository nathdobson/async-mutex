use test::Bencher;
use futures::executor::ThreadPool;
use futures::task::{SpawnExt, Spawn};
use crate::{Mutex, spin_lock, fair_mutex, unfair_mutex};
use crate::loom::sync::Arc;
use std::mem;
use crate::util::yield_now;
use crate::future::{block_on, Future};
use futures::future::{join_all};
use std::ops::DerefMut;
use tokio::runtime::Runtime;
use std::time::Instant;
use std::any::type_name;
use crate::future::poll_fn;
use futures::pin_mut;

// async fn step(mut guard: impl DerefMut<Target=usize>) {
//     *guard += 1;
//     mem::drop(guard);
// }
//
// trait IsMutex {
//     fn new() -> Self;
//     fn add(self: Arc<Self>, steps: usize) -> tokio::task::JoinHandle<()>;
// }


// fn bench_mutex<T: IsMutex>() {
//     let tasks = 500000usize;
//     let count = 10usize;
//     let rt = Runtime::new().unwrap();
//     let rte = rt.enter();
//     let cpu = cpu_time::ProcessTime::now();
//     let time = Instant::now();
//     let mutex = Arc::new(T::new());
//     block_on(join_all((0..tasks).map(|task| mutex.clone().add(count))));
//     let time = time.elapsed();
//     let cpu = cpu.elapsed();
//     println!("{:40} time = {:20} cpu = {:20} ratio = {:20}", type_name::<T>(), format!("{:?}", time), format!("{:?}", cpu), format!("{:?}", cpu.as_secs_f64() / time.as_secs_f64()));
// }
//
// impl IsMutex for Mutex<usize> {
//     fn new() -> Self {
//         Mutex::new(0)
//     }
//     fn add(self: Arc<Self>, steps: usize) -> tokio::task::JoinHandle<()> {
//         tokio::spawn(async move {
//             for i in 0..steps {
//                 step(self.lock().await).await;
//             }
//         })
//     }
// }
//
// impl IsMutex for tokio::sync::Mutex<usize> {
//     fn new() -> Self {
//         tokio::sync::Mutex::new(0)
//     }
//     fn add(self: Arc<Self>, steps: usize) -> tokio::task::JoinHandle<()> {
//         tokio::spawn(async move {
//             for i in 0..steps {
//                 step(self.lock().await).await;
//             }
//         })
//     }
// }
//
// impl IsMutex for async_std::sync::Mutex<usize> {
//     fn new() -> Self {
//         Self::new(0)
//     }
//     fn add(self: Arc<Self>, steps: usize) -> tokio::task::JoinHandle<()> {
//         tokio::spawn(async move {
//             for i in 0..steps {
//                 step(self.lock().await).await;
//             }
//         })
//     }
// }
//
// impl IsMutex for futures::lock::Mutex<usize> {
//     fn new() -> Self {
//         Self::new(0)
//     }
//     fn add(self: Arc<Self>, steps: usize) -> tokio::task::JoinHandle<()> {
//         tokio::spawn(async move {
//             for i in 0..steps {
//                 step(self.lock().await).await;
//             }
//         })
//     }
// }
//
// impl IsMutex for async_mutex::Mutex<usize> {
//     fn new() -> Self {
//         Self::new(0)
//     }
//     fn add(self: Arc<Self>, steps: usize) -> tokio::task::JoinHandle<()> {
//         tokio::spawn(async move {
//             for i in 0..steps {
//                 step(self.lock().await).await;
//             }
//         })
//     }
// }
//
// impl IsMutex for futures_locks::Mutex<usize> {
//     fn new() -> Self {
//         Self::new(0)
//     }
//     fn add(self: Arc<Self>, steps: usize) -> tokio::task::JoinHandle<()> {
//         tokio::spawn(async move {
//             for i in 0..steps {
//                 step(self.lock().await).await;
//             }
//         })
//     }
// }

trait Lock<'a, M: 'a>: Fn<(&'a M, ), Output: Send + Future<Output: DerefMut<Target=usize>>> {}

impl<'a, M: 'a, F> Lock<'a, M> for F where F: Fn<(&'a M, ), Output: Send + Future<Output: DerefMut<Target=usize>>> {}

async fn poll_count(fut: impl Future<Output=()>) -> usize {
    let mut count = 0;
    pin_mut!(fut);
    poll_fn(|cx| {
        count += 1;
        fut.as_mut().poll(cx)
    }).await;
    count
}

fn bench_mutex<N, L>(name: &str, mut new: N, lock: L)
    where
        N: FnMut<(usize, )>,
        N::Output: 'static + Send + Sync,
        L: 'static + Copy + Send + Sync + for<'a> Lock<'a, N::Output>
{
    let tasks = 100000usize;
    let count = 10usize;
    let cpu = cpu_time::ProcessTime::now();
    let time = Instant::now();
    let mutex = Arc::new(new(0));
    let polls = block_on(join_all((0..tasks).map(|task| {
        let mutex = mutex.clone();
        tokio::spawn(poll_count(async move {
            for i in 0..count {
                *lock(&mutex).await += 1;
            }
        }))
    }))).into_iter().sum::<Result<usize, _>>().unwrap();
    let time = time.elapsed();
    let cpu = cpu.elapsed();
    println!("{:40} time = {:20} cpu = {:20} ratio = {:20} polls = {:?}",
             name,
             format!("{:?}", time),
             format!("{:?}", cpu),
             format!("{:?}", cpu.as_secs_f64() / time.as_secs_f64()),
             polls);
}

#[test]
#[ignore]
fn bench() {
    let rt = Runtime::new().unwrap();
    let rte = rt.enter();
    bench_mutex("crate::fair_mutex", fair_mutex::Mutex::new, fair_mutex::Mutex::lock);
    bench_mutex("crate::unfair_mutex", unfair_mutex::Mutex::new, unfair_mutex::Mutex::lock);
    bench_mutex("crate::spin_lock", spin_lock::Mutex::new, spin_lock::Mutex::lock);
    bench_mutex("tokio::sync", tokio::sync::Mutex::new, tokio::sync::Mutex::lock);
    bench_mutex("async_std::sync", async_std::sync::Mutex::new, async_std::sync::Mutex::lock);
    //bench_mutex("futures_util::lock", futures_util::lock::Mutex::new, futures_util::lock::Mutex::lock);
    bench_mutex("async_mutex", async_mutex::Mutex::new, async_mutex::Mutex::lock);
    bench_mutex("futures_locks", futures_locks::Mutex::new, futures_locks::Mutex::lock);
}

