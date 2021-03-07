use futures::executor::{LocalPool, block_on};
use crate::test_waker::TestWaker;
use crate::{Mutex, MutexGuard, MutexScope};
use std::task::{Context, Poll};
use crate::util::{FnOnceExt, yield_now};
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

struct Test<Start, Run, Stop>
    where Start: Send + Sync + 'static + Fn<()>,
          Start::Output: Send + Sync + 'static,
          Run: Send + Sync + 'static + Fn<(Arc<Start::Output>, usize)>,
          Run::Output: Future + Send + 'static,
          <Run::Output as Future>::Output: Send + 'static,
          Stop: Send + Sync + 'static + Fn(Start::Output, Vec<<Run::Output as Future>::Output>)

{
    tasks: usize,
    start: Start,
    run: Run,
    stop: Stop,
}

trait IsTest: Send + Sync + 'static {
    type State: Send + Sync + 'static;
    type Output: Send + 'static;
    type RunFut: Future<Output=Self::Output> + Send;
    fn tasks(&self) -> usize;
    fn start(&self) -> Self::State;
    fn run(&self, state: Arc<Self::State>, task: usize) -> Self::RunFut;
    fn stop(&self, state: Self::State, outputs: Vec<<Self::RunFut as Future>::Output>);
}

impl<Start, Run, Stop> IsTest for Test<Start, Run, Stop>
    where Start: Send + Sync + 'static + Fn<()>,
          Start::Output: Send + Sync + 'static,
          Run: Send + Sync + 'static + Fn<(Arc<Start::Output>, usize)>,
          Run::Output: Future + Send + 'static,
          <Run::Output as Future>::Output: Send + 'static,
          Stop: Send + Sync + 'static + Fn(Start::Output, Vec<<Run::Output as Future>::Output>)
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
    fn stop(&self, state: Self::State, outputs: Vec<<Self::RunFut as Future>::Output>) {
        (self.stop)(state, outputs);
    }
}

fn run_locally<T: IsTest>(test: T) {
    run_locally_arc(Arc::new(test));
}

fn run_threaded<T: IsTest>(test: T) {
    run_threaded_arc(Arc::new(test));
}

fn run_locally_arc<T: IsTest>(test: Arc<T>) {
    let mut pool = LocalPool::new();
    let spawner = pool.spawner();
    let fut = run_with_spawner_arc(test, &spawner);
    pool.run();
    block_on(fut);
}

fn run_threaded_arc<T: IsTest>(test: Arc<T>) {
    let mut pool = ThreadPool::new().unwrap();
    let fut = run_with_spawner_arc(test, &pool);
    block_on(fut);
}

fn run_with_spawner_arc<T: IsTest>(test: Arc<T>, spawner: &dyn Spawn) -> impl Future {
    let state = Arc::new(test.start());

    let mut tasks: Vec<_> = (0..test.tasks()).collect();
    tasks.shuffle(&mut thread_rng());
    let joined = join_all(tasks.into_iter().map(|task| {
        let state = state.clone();
        let test = test.clone();
        spawner.spawn_with_handle(async move {
            test.run(state, task).await
        }).unwrap()
    }));
    async move {
        let outputs = joined.await;
        test.stop(Arc::try_unwrap(state).ok().unwrap(), outputs);
    }
}

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
                        || line.starts_with("use std::cell"){
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

fn static_test() {
    async fn imp() {
        let mutex = Mutex::new(0usize);
        let mut x = 1usize;
        let scope = mutex.scope();
        scope.with(async {
            let guard = scope.lock().await;
            yield_now().await;
            mem::drop(guard);
        }).await;
    }

    fn assert_send<X: Send>(x: X) {}

    fn assert_test2_send() {
        assert_send(imp())
    }
}

#[test]
fn test_lock1() {
    let (test_waker, waker) = TestWaker::new();
    let mut cx = Context::from_waker(&waker, false);
    assert_eq!((1, 0), test_waker.load());
    let mutex = Mutex::new(1usize);
    {
        let fut = async {
            let scope = mutex.scope();
            scope.with(async {
                let mut guard = scope.lock().await;
                ready(()).await;
                *guard += 1;
                *guard
            }).await
        };
        pin_mut!(fut);
        assert_eq!(Poll::Ready(2), fut.poll(&mut cx));
    }
    assert_eq!((1, 0), test_waker.load());
}

#[test]
fn test_lock2() {
    let (test_waker1, waker1) = TestWaker::new();
    let (test_waker2, waker2) = TestWaker::new();
    let mut cx1 = Context::from_waker(&waker1, false);
    let mut cx2 = Context::from_waker(&waker2, false);
    assert_eq!((1, 0), test_waker1.load());
    assert_eq!((1, 0), test_waker2.load());
    let mutex = Mutex::new(1usize);
    {
        async fn add_fetch(mutex: &Mutex<usize>) -> usize {
            let scope = mutex.scope();
            scope.with(async {
                let mut guard = scope.lock().await;
                *guard += 1;
                yield_now().await;
                *guard
            }).await
        }
        let fut1 = add_fetch(&mutex);
        let fut2 = add_fetch(&mutex);
        pin_mut!(fut1);
        pin_mut!(fut2);

        assert_eq!(Poll::Pending, fut1.as_mut().poll(&mut cx1));
        assert_eq!((2, 1), test_waker1.load());
        assert_eq!((1, 0), test_waker2.load());

        assert_eq!(Poll::Pending, fut2.as_mut().poll(&mut cx2));
        assert_eq!((2, 1), test_waker1.load());
        assert_eq!((2, 0), test_waker2.load());

        assert_eq!(Poll::Ready(2), fut1.as_mut().poll(&mut cx1));
        assert_eq!((1, 1), test_waker1.load());
        assert_eq!((1, 1), test_waker2.load());

        assert_eq!(Poll::Pending, fut2.as_mut().poll(&mut cx2));
        assert_eq!((1, 1), test_waker1.load());
        assert_eq!((2, 2), test_waker2.load());

        assert_eq!(Poll::Ready(3), fut2.as_mut().poll(&mut cx2));
        assert_eq!((1, 1), test_waker1.load());
        assert_eq!((1, 2), test_waker2.load());
    }
    assert_eq!((1, 1), test_waker1.load());
    assert_eq!((1, 2), test_waker2.load());
}

#[test]
fn test_lock_simple() {
    let tasks = 4;
    run_threaded(Test {
        tasks,
        start: move || -> Mutex<usize>{
            Mutex::new(0usize)
        },
        run: move |mutex, task| async move {
            let scope = mutex.scope();
            scope.with(async {
                loop {
                    println!("Yielding {:?}", task);
                    yield_now().await;
                    println!("Locking {:?}", task);
                    let mut lock = scope.lock().await;
                    println!("Locked {:?}", task);
                    if *lock == task {
                        *lock = task + 1;
                        return;
                    }
                    println!("Unlocking {:?}", task);
                    mem::drop(lock);
                    println!("Unlocked {:?}", task);
                }
            }).await;
        },
        stop: move |mut mutex, _| {
            assert_eq!(tasks, *mutex.get_mut())
        },
    })
}