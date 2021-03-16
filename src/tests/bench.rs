#![allow(unused_parens)]

use test::Bencher;
use futures::executor::ThreadPool;
use futures::task::{SpawnExt, Spawn};
use crate::{Mutex, spin_lock, fair_mutex};
use crate::loom::sync::Arc;
use std::mem;
use crate::util::yield_now;
use crate::future::{block_on, Future};
use futures::future::{join_all};
use std::ops::{DerefMut};
use tokio::runtime::Runtime;
use std::time::Instant;
use std::any::type_name;
use crate::future::poll_fn;
use futures::{pin_mut, FutureExt};
use crate::mpsc::channel;
use std::mem::size_of;
use std::marker::PhantomData;
use std::any::Any;
//use crate::async_traits::async_traits;
use std::sync::mpsc::SendError;
use std::sync::mpsc::RecvError;
use std::default::default;
use futures::SinkExt;

async_traits! {
    pub trait SenderTrait<M>: Send PLUS Sync PLUS 'static{
        name sender_trait;
        async fn send<'a>(&'a self, msg:M) -> (Result<(), SendError<M>>);
    }
    pub trait ReceiverTrait<M> : 'static PLUS Send{
        name receiver_trait;
        async fn recv<'a>(&'a mut self) -> (Result<M, RecvError>);
    }

    impl<M: Send PLUS 'static> SenderTrait<M> for crate::mpsc::Sender<M> as crate_sender {
        name sender_trait;
        async fn send = crate::mpsc::Sender::send;
    }
    impl<M: Send PLUS 'static> ReceiverTrait<M> for crate::mpsc::Receiver<M> as crate_receiver {
        name receiver_trait;
        async fn recv = crate::mpsc::Receiver::recv;
    }

    impl<M: Send PLUS 'static> SenderTrait<M> for tokio::sync::mpsc::Sender<M> as tokio_sender {
        name sender_trait;
        async fn send = {
            async fn send<M>(sender:&tokio::sync::mpsc::Sender<M>, message:M)->Result<(),SendError<M>>{
                sender.send(message).await.map_err(|e|SendError(e.0))
            }
            send
        };
    }
    impl<M: Send PLUS 'static> ReceiverTrait<M> for tokio::sync::mpsc::Receiver<M> as tokio_receiver {
        name receiver_trait;
        async fn recv = {
            async fn recv<M>(receiver:&mut tokio::sync::mpsc::Receiver<M>) -> Result<M, RecvError> {
                receiver.recv().await.ok_or(RecvError)
            }
            recv
        };
    }

    impl<M: Send PLUS 'static> SenderTrait<M> for async_channel::Sender<M> as async_channel_sender {
        name sender_trait;
        async fn send = {
            async fn send<M>(sender:&async_channel::Sender<M>, message:M)->Result<(),SendError<M>>{
                sender.send(message).await.map_err(|e|SendError(e.0))
            }
            send
        };
    }
    impl<M: Send PLUS 'static> ReceiverTrait<M> for async_channel::Receiver<M> as async_channel_receiver {
        name receiver_trait;
        async fn recv = {
            async fn recv<M>(receiver:&mut async_channel::Receiver<M>) -> Result<M, RecvError> {
                receiver.recv().await.map_err(|_| RecvError)
            }
            recv
        };
    }

    impl<M: Send PLUS 'static> SenderTrait<M> for async_std::channel::Sender<M> as async_std_sender {
        name sender_trait;
        async fn send = {
            async fn send<M>(sender:&async_std::channel::Sender<M>, message:M)->Result<(),SendError<M>>{
                Ok(sender.send(message).await.unwrap())
            }
            send
        };
    }
    impl<M: Send PLUS 'static> ReceiverTrait<M> for async_std::channel::Receiver<M> as async_std_receiver {
        name receiver_trait;
        async fn recv = {
            async fn recv<M>(receiver:&mut async_std::channel::Receiver<M>) -> Result<M, RecvError> {
                receiver.recv().await.map_err(|_| RecvError)
            }
            recv
        };
    }

    // impl<M: Send PLUS 'static> SenderTrait<M> for futures::channel::mpsc::Sender<M> as futures_sender {
    //     name sender_trait;
    //     async fn send = {
    //         async fn send<M>(sender:&futures::channel::mpsc::Sender<M>, message:M)->Result<(),SendError<M>>{
    //             Ok(sender.send(message).await.unwrap())
    //         }
    //         send
    //     };
    // }
    // impl<M: Send PLUS 'static> ReceiverTrait<M> for futures::channel::mpsc::Receiver<M> as futures_receiver {
    //     name receiver_trait;
    //     async fn recv = {
    //         async fn recv<M>(receiver:&mut futures::channel::mpsc::Receiver<M>) -> Result<M, RecvError> {
    //             receiver.recv().await.map_err(|_| RecvError)
    //         }
    //         recv
    //     };
    // }

    pub trait MutexTrait<M>: Send PLUS Sync PLUS 'static{
        name mutex_trait;
        async fn lock<'a>(&'a self) IMPL -> impl (std::ops::DerefMut<Target=M>);
    }
    impl<M: Send PLUS 'static> MutexTrait<M> for crate::Mutex<M> as crate_mutex {
        name mutex_trait;
        async fn lock = crate::Mutex::lock;
    }
    impl<M: Send PLUS 'static> MutexTrait<M> for crate::spin_lock::Mutex<M> as spin_lock_mutex {
        name mutex_trait;
        async fn lock = crate::spin_lock::Mutex::lock;
    }
    impl<M: Send PLUS 'static> MutexTrait<M> for tokio::sync::Mutex<M> as tokio_mutex {
        name mutex_trait;
        async fn lock = tokio::sync::Mutex::lock;
    }
    impl<M: Send PLUS 'static> MutexTrait<M> for async_std::sync::Mutex<M> as async_std_mutex {
        name mutex_trait;
        async fn lock = async_std::sync::Mutex::lock;
    }
    impl<M: Send PLUS 'static> MutexTrait<M> for async_mutex::Mutex<M> as async_mutex_mutex {
        name mutex_trait;
        async fn lock = async_mutex::Mutex::lock;
    }
    impl<M: Send PLUS 'static> MutexTrait<M> for futures_locks::Mutex<M> as futures_locks_mutex {
        name mutex_trait;
        async fn lock = futures_locks::Mutex::lock;
    }
}

fn crate_channel<M: Send + 'static>(cap: usize) -> (impl SenderTrait<M>, impl ReceiverTrait<M>) {
    let (s, r) = crate::mpsc::channel(cap);
    (crate_sender(s), crate_receiver(r))
}

fn tokio_channel<M: Send + 'static>(cap: usize) -> (impl SenderTrait<M>, impl ReceiverTrait<M>) {
    let (s, r) = tokio::sync::mpsc::channel(cap);
    (tokio_sender(s), tokio_receiver(r))
}

fn async_channel_channel<M: Send + 'static>(cap: usize) -> (impl SenderTrait<M>, impl ReceiverTrait<M>) {
    let (s, r) = async_channel::bounded(cap);
    (async_channel_sender(s), async_channel_receiver(r))
}

fn async_std_channel<M: Send + 'static>(cap: usize) -> (impl SenderTrait<M>, impl ReceiverTrait<M>) {
    let (s, r) = async_std::channel::bounded(cap);
    (async_std_sender(s), async_std_receiver(r))
}

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

fn bench_mutex(name: &str, mutex: impl MutexTrait<usize>) {
    let tasks = 100000usize;
    let count = 10usize;
    let mutex = Arc::new(mutex);
    run_bench(name, (0..tasks).map(move |task| {
        let mutex = mutex.clone();
        async move {
            for i in 0..count {
                *mutex.lock().await += 1;
            }
        }
    }));
}

const CHANNEL_CAP: usize = 1000;
const CHANNEL_COUNT: usize = 1000000;

fn bench_channel(name: &str, (sender, mut receiver): (impl SenderTrait<usize>, impl ReceiverTrait<usize>)) {
    let sender = Arc::new(sender);
    run_bench(name, vec![
        {
            let sender = sender.clone();
            async move {
                for i in 0..CHANNEL_COUNT {
                    sender.send(i).await.unwrap();
                }
            }
        }.boxed(), {
            let sender = sender.clone();
            async move {
                for i in 0..CHANNEL_COUNT {
                    sender.send(i).await.unwrap();
                }
            }
        }.boxed(), async move {
            for i in 0..CHANNEL_COUNT * 2 {
                receiver.recv().await.unwrap();
            }
        }.boxed()].into_iter())
}

#[test]
#[ignore]
fn bench_mutexes() {
    let rt = Runtime::new().unwrap();
    let rte = rt.enter();
    bench_mutex("fair_mutex", crate_mutex(default()));
    bench_mutex("spin_lock", spin_lock_mutex(default()));
    bench_mutex("tokio", tokio_mutex(default()));
    bench_mutex("async_std", async_std_mutex(default()));
    //bench_mutex("futures_util::lock", futures_util::lock::Mutex::new, futures_util::lock::Mutex::lock);
    bench_mutex("async_mutex", async_mutex_mutex(default()));
    bench_mutex("futures", futures_locks_mutex(default()));
}

#[test]
#[ignore]
fn bench_channels() {
    let rt = Runtime::new().unwrap();
    let rte = rt.enter();
    bench_channel("crate", crate_channel(CHANNEL_CAP));
    bench_channel("tokio", tokio_channel(CHANNEL_CAP));
    bench_channel("async_channel", async_channel_channel(CHANNEL_CAP));
    bench_channel("async_std", async_std_channel(CHANNEL_CAP));
    //bench_mutex("futures", futures_channel(CHANNEL_CAP));
}