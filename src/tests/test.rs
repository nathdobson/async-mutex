use crate::future::Future;
use crate::sync::Arc;
use futures::executor::{LocalPool, ThreadPool};
use crate::future::block_on;
use futures::task::{Spawn, SpawnExt};
use rand::thread_rng;
use rand::seq::SliceRandom;
use futures::future::join_all;
use crate::Mutex;
use crate::cancel::Cancel;
use crate::test_println;
use futures::future::poll_fn;
use futures::pin_mut;
use std::task::Poll;
use crate::tests::now_or_cancel;

#[derive(Copy, Clone)]
pub struct Test<Start, Run, Stop>
    where Start: Send + Sync + 'static + Fn<()>,
          Start::Output: Send + Sync + 'static,
          Run: Send + Sync + 'static + Fn<(Arc<Start::Output>, usize)>,
          Run::Output: Future + Send + 'static,
          <Run::Output as Future>::Output: Send + 'static,
          Stop: Send + Sync + 'static + Fn(&mut Start::Output, Vec<<Run::Output as Future>::Output>) {
    tasks: usize,
    start: Start,
    run: Run,
    stop: Stop,
}

pub trait IsTest: Send + Sync + 'static + Copy {
    type State: Send + Sync + 'static;
    type Output: Send + 'static;
    type RunFut: Future<Output=Self::Output> + Send;
    fn tasks(&self) -> usize;
    fn start(&self) -> Self::State;
    fn run(&self, state: Arc<Self::State>, task: usize) -> Self::RunFut;
    fn stop(&self, state: &mut Self::State, outputs: Vec<<Self::RunFut as Future>::Output>);
}

impl<Start: Copy, Run: Copy, Stop: Copy> IsTest for Test<Start, Run, Stop>
    where Start: Send + Sync + 'static + Fn<()>,
          Start::Output: Send + Sync + 'static,
          Run: Send + Sync + 'static + Fn<(Arc<Start::Output>, usize)>,
          Run::Output: Future + Send + 'static,
          <Run::Output as Future>::Output: Send + 'static,
          Stop: Send + Sync + 'static + Fn(&mut Start::Output, Vec<<Run::Output as Future>::Output>)
{
    type State = Start::Output;
    type Output = <Run::Output as Future>::Output;
    type RunFut = Run::Output;
    fn tasks(&self) -> usize {
        self.tasks
    }
    fn start(&self) -> Self::State {
        (self.start)()
    }
    fn run(&self, state: Arc<Self::State>, task: usize) -> Self::RunFut {
        (self.run)(state, task)
    }
    fn stop(&self, state: &mut Self::State, outputs: Vec<<Self::RunFut as Future>::Output>) {
        (self.stop)(state, outputs);
    }
}

pub fn simple_test(tasks: usize) -> impl IsTest {
    Test {
        tasks,
        start: move || -> Mutex<usize>{
            test_println!("Creating mutex");
            Mutex::new(0)
        },
        run: move |mutex, task| async move {
            let cancel = Cancel::new();
            let scope = mutex.scope();
            scope.with(async {
                test_println!("Starting {}", task);
                let mut lock = scope.lock(&cancel).await;
                test_println!("Running  {}", task);
                *lock |= task;
            }).await;
        },
        stop: move |mut mutex, _| {
            let mut expected = 0;
            for task in 0..tasks {
                expected |= task;
            }
            assert_eq!(expected, *mutex.get_mut())
        },
    }
}

pub fn cancel_test(locks: usize, cancels: usize) -> impl IsTest {
    Test {
        tasks: locks + cancels,
        start: move || -> Mutex<usize>{
            test_println!("Creating mutex");
            Mutex::new(0)
        },
        run: move |mutex, task| async move {
            let cancel = Cancel::new();
            let scope = mutex.scope();
            scope.with(async {
                if task < locks {
                    test_println!("Locking    {}", task);
                    let mut lock = scope.lock(&cancel).await;
                    test_println!("Running    {}", task);
                    *lock |= task;
                } else {
                    test_println!("Canceling  {}", task);
                    if let Ok(guard) = now_or_cancel(&cancel, scope.lock(&cancel)).await {
                        test_println!("Not cancel {}", task);
                    } else {
                        test_println!("Cancel     {}", task);
                    }
                }
            }).await;
        },
        stop: move |mut mutex, _| {
            let mut expected = 0;
            for task in 0..locks {
                expected |= task;
            }
            assert_eq!(expected, *mutex.get_mut())
        },
    }
}