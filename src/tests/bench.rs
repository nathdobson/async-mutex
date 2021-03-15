use test::Bencher;
use futures::executor::ThreadPool;
use futures::task::{SpawnExt, Spawn};
use crate::{Mutex, spin_lock, fair_mutex};
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
use futures::{pin_mut, FutureExt};
use crate::mpsc::channel;
use std::mem::size_of;
use std::marker::PhantomData;
use std::any::Any;

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

fn run_bench<I>(name: &str, iter: I) where I: Iterator<Item: 'static + Send + Future<Output=()>> {
    let cpu = cpu_time::ProcessTime::now();
    let time = Instant::now();
    let polls = block_on(join_all(iter.map(|task| {
        tokio::spawn(poll_count(task))
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

fn bench_mutex<N, L>(name: &str, mut new: N, lock: L)
    where
        N: FnMut<(usize, )>,
        N::Output: 'static + Send + Sync,
        L: 'static + Copy + Send + Sync + for<'a> Lock<'a, N::Output>
{
    let tasks = 100000usize;
    let count = 10usize;
    let mutex = Arc::new(new(0));
    run_bench(name, (0..tasks).map(move |task| {
        let mutex = mutex.clone();
        async move {
            for i in 0..count {
                *lock(&mutex).await += 1;
            }
        }
    }));
}

//
// fn make_sender<S, SendImpl, SendImplValue>(
//     s: S, send_impl: SendImpl,
// ) -> impl Sender
//     where SendImpl: Copy + Clone + Send + Sync + 'static + for<'a> SendFn<'a, S>,
//           SendImplValue: Unique<SendImpl> {
//     #[repr(transparent)]
//     struct Result<S, SendImpl, SendImplValue> (S, PhantomData<SendImpl>, PhantomData<SendImplValue>);
//     impl<S, SendImpl, SendImplValue> Sender for Result<S> {
//         type SendImpl = SendImpl;
//         fn send_impl() -> Self::SendImpl {
//             &self.1
//         }
//     }
// }


// fn bench_channel<S, R, SendImpl, RecvImpl>(
//     name: &str,
//     new: impl Fn(usize) -> (S, R),
//     send: SendImpl,
//     recv: RecvImpl)
//     where
//         S: Clone + Send + Sync + 'static,
//         R: Send + Sync + 'static,
//         SendImpl: Copy + Clone + Send + Sync + 'static + for<'a> SendFn<'a, S>,
//         RecvImpl: Copy + Clone + Send + Sync + 'static + for<'a> RecvFn<'a, R>,
// {
//     let cap = 1000;
//     let count: usize = 1000000;
//     let (sender, mut receiver) = new(10);
//     run_bench(name, vec![
//         {
//             let sender = sender.clone();
//             async move {
//                 for i in 0..count {
//                     send(&sender, i).await;
//                 }
//             }
//         }.boxed(), {
//             let sender = sender.clone();
//             async move {
//                 for i in 0..count {
//                     send(&sender, i).await;
//                 }
//             }
//         }.boxed(), async move {
//             for i in 0..count * 2 {
//                 recv(&mut receiver).await;
//             }
//         }.boxed()].into_iter())
// }

#[test]
#[ignore]
fn bench_mutexes() {
    let rt = Runtime::new().unwrap();
    let rte = rt.enter();
    bench_mutex("crate::fair_mutex", fair_mutex::Mutex::new, fair_mutex::Mutex::lock);
    //bench_mutex("crate::unfair_mutex", unfair_mutex::Mutex::new, unfair_mutex::Mutex::lock);
    bench_mutex("crate::spin_lock", spin_lock::Mutex::new, spin_lock::Mutex::lock);
    bench_mutex("tokio::sync", tokio::sync::Mutex::new, tokio::sync::Mutex::lock);
    bench_mutex("async_std::sync", async_std::sync::Mutex::new, async_std::sync::Mutex::lock);
    //bench_mutex("futures_util::lock", futures_util::lock::Mutex::new, futures_util::lock::Mutex::lock);
    bench_mutex("async_mutex", async_mutex::Mutex::new, async_mutex::Mutex::lock);
    bench_mutex("futures_locks", futures_locks::Mutex::new, futures_locks::Mutex::lock);
}

#[test]
#[ignore]
fn bench_channels() {
    let rt = Runtime::new().unwrap();
    let rte = rt.enter();
    //bench_channel("crate::mpsc", channel, crate::mpsc::Sender::send, crate::mpsc::Receiver::recv);
    //bench_channel("tokio::sync::mpsc", tokio::sync::mpsc::channel, tokio::sync::mpsc::Sender::send, tokio::sync::mpsc::Receiver::recv);
}