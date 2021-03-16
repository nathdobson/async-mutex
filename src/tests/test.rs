use crate::future::Future;
use crate::sync::Arc;
use futures::executor::{LocalPool, ThreadPool};
use crate::future::block_on;
use futures::task::{Spawn, SpawnExt};
use futures::future::join_all;
use crate::Mutex;
use crate::test_println;
use futures::future::poll_fn;
use futures::pin_mut;
use std::task::Poll;
use futures::poll;
use std::mem;
use crate::condvar::Condvar;
use std::collections::HashSet;
use crate::mpsc;
use crate::mpsc::Receiver;
use crate::mpsc::Sender;
use std::mem::MaybeUninit;
use crate::cell::UnsafeCell;
use crate::rwlock::RwLock;

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
            test_println!("Starting {}", task);
            let mut lock = mutex.lock().await;
            test_println!("Running  {}", task);
            *lock |= task;
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
            if task < locks {
                test_println!("Locking    {}", task);
                let mut lock = mutex.lock().await;
                test_println!("Running    {}", task);
                *lock |= task;
            } else {
                test_println!("Canceling  {}", task);
                let fut = mutex.lock();
                pin_mut!(fut);
                mem::drop(poll!(fut));
            }
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


pub fn condvar_test_a() -> impl IsTest {
    Test {
        tasks: 3,
        start: move || -> (Mutex<HashSet<usize>>, Condvar){
            test_println!("Creating mutex");
            (Mutex::new(HashSet::new()), Condvar::new())
        },
        run: move |pair: Arc<(Mutex<HashSet<usize>>, Condvar)>, task| async move {
            if task == 0 || task == 1 {
                test_println!("Locking {:?}", task);
                let mut lock = pair.0.lock().await;
                test_println!("Inserting {:?}", task);
                lock.insert(task);
                test_println!("Notifying {:?}", task);
                pair.1.notify(&mut lock, 1);
                test_println!("Dropping {:?}", task);
                mem::drop(lock);
                test_println!("Done {:?}", task);
            } else {
                test_println!("Locking {:?}", task);
                let mut lock = pair.0.lock().await;
                test_println!("Checking {:?}", task);
                while *lock != (0..2).collect() {
                    lock = pair.1.wait(lock).await;
                    test_println!("Waken {:?}", task);
                }
                test_println!("Dropping {:?}", task);
                mem::drop(lock);
                test_println!("Done {:?}", task);
            }
        },
        stop: move |mut pair, _| {},
    }
}

pub fn condvar_test(count: usize) -> impl IsTest {
    Test {
        tasks: count * 2,
        start: move || -> (Mutex<HashSet<usize>>, Condvar){
            test_println!("Creating mutex");
            (Mutex::new(HashSet::new()), Condvar::new())
        },
        run: move |pair: Arc<(Mutex<HashSet<usize>>, Condvar)>, task| async move {
            let index = task / 2;
            if task % 2 == 0 {
                test_println!("Locking {:?}", task);
                let mut lock = pair.0.lock().await;
                test_println!("Inserting {:?}", task);
                lock.insert(index);
                pair.1.notify_all(&mut lock);
                test_println!("Unlocking {:?}", task);
                mem::drop(lock);
                test_println!("Done {:?}", task);
            } else {
                test_println!("Locking {:?}", task);
                let mut lock = pair.0.lock().await;
                test_println!("Checking {:?}", task);
                while !lock.contains(&index) {
                    lock = pair.1.wait(lock).await;
                    test_println!("Waited {:?}", task);
                }
                test_println!("Unlocking {:?}", task);
                mem::drop(lock);
                test_println!("Done {:?}", task);
            }
        },
        stop: move |mut pair, _| {
            assert_eq!((0..count).collect::<HashSet<_>>(), *pair.0.get_mut());
        },
    }
}

pub fn channel_test(senders: &'static [usize], cap: usize) -> impl IsTest {
    #[derive(Debug, Clone)]
    struct Message {
        task: usize,
        seq: Box<usize>,
    }
    struct ReceiverBox(UnsafeCell<Option<Receiver<Message>>>);
    unsafe impl Sync for ReceiverBox {}
    unsafe impl Send for ReceiverBox {}

    Test {
        tasks: senders.len() + 1,
        start: move || -> (Sender<Message>, ReceiverBox){
            test_println!("Creating channel");
            let (sender, receiver) = mpsc::channel(cap);
            (sender, ReceiverBox(UnsafeCell::new(Some(receiver))))
        },
        run: move |pair: Arc<(Sender<Message>, ReceiverBox)>, task| async move {
            if task == senders.len() {
                let mut receiver = unsafe { pair.1.0.with_mut(|x| (*x).take()).unwrap() };
                let mut received = vec![vec![]; senders.len()];
                for i in 0..senders.iter().sum::<usize>() {
                    let message = receiver.recv().await.unwrap();
                    test_println!("Received {:?}", message);
                    received[message.task].push(*message.seq);
                }
                let expected: Vec<Vec<usize>> = senders.iter().map(|x| (0..*x).collect()).collect();
            } else {
                for seq in 0..senders[task] {
                    let msg = Message { task, seq: Box::new(seq) };
                    test_println!("Sending {:?}", msg);
                    pair.0.send(msg.clone()).await.unwrap();
                    test_println!("Sent {:?}", msg);
                }
            }
        },
        stop: move |mut pair, _| {},
    }
}


pub fn rwlock_test(writers: usize, readers: usize) -> impl IsTest {
    Test {
        tasks: readers + writers,
        start: move || -> RwLock<usize>{
            test_println!("Creating rwlock");
            RwLock::new(0)
        },
        run: move |rwlock: Arc<RwLock<usize>>, task| async move {
            if task < writers {
                *rwlock.write().await ^= task;
            } else {
                let value = *rwlock.read().await;
                test_println!("{:?}",value );
            }
        },
        stop: move |mut mutex, _| {
            let mut expected = 0;
            for task in 0..writers {
                expected ^= task;
            }
            assert_eq!(expected, *mutex.get_mut())
        },
    }
}
