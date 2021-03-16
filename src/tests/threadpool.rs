use futures::future::FutureObj;
use futures::task::{Spawn, SpawnError, SpawnExt};
use std::sync::Mutex;
use rand::seq::SliceRandom;
use rand::thread_rng;
use itertools::Itertools;
use crate::thread;
use futures::executor::LocalPool;
use crate::sync::Arc;
use crate::sync::Barrier;

pub struct ThreadPool {
    futures: Mutex<Vec<FutureObj<'static, ()>>>,
}

impl ThreadPool {
    pub fn new() -> Self {
        ThreadPool { futures: Mutex::new(vec![]) }
    }
    pub fn run(self, count: usize) {
        let mut futures = self.futures.into_inner().unwrap();
        futures.shuffle(&mut thread_rng());
        let mut by_thread: Vec<Vec<_>> = (0..count).map(|_| vec![]).collect();
        for chunk in futures.into_iter().chunks(count).into_iter() {
            for (index, future) in chunk.enumerate() {
                by_thread[index].push(future);
            }
        }
        let barrier = Arc::new(Barrier::new(count));
        by_thread.into_iter().map(|futures| {
            let barrier = barrier.clone();
            thread::spawn(move || {
                let mut pool = LocalPool::new();
                let spawner = pool.spawner();
                for future in futures {
                    spawner.spawn_obj(future).unwrap();
                }
                barrier.wait();
                pool.run();
            })
        }).collect::<Vec<_>>().into_iter().for_each(|handle| handle.join().unwrap());
    }
}

impl Spawn for ThreadPool {
    fn spawn_obj(&self, future: FutureObj<'static, ()>) -> Result<(), SpawnError> {
        self.futures.lock().unwrap().push(future);
        Ok(())
    }
}