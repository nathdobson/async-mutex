use crate::{thread, Mutex};
use crate::sync::Arc;
use crate::tests::test::{IsTest, simple_test, cancel_test};
use futures::executor::{LocalPool, ThreadPool};
use crate::future::{block_on, Future, ready, poll_fn};
use futures::task::{Spawn, SpawnExt};
use rand::thread_rng;
use rand::seq::SliceRandom;
use futures::future::join_all;
use std::time::Duration;
use crate::thread::sleep;
use std::mem;
use std::task::{Context, Poll};
use crate::util::yield_now;
use futures::pin_mut;
use crate::tests::test_waker::TestWaker;

fn run_with_spawner<T: IsTest>(test: T, spawner: &dyn Spawn) -> impl Future {
    let mut state = Arc::new(test.start());

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
        test.stop(Arc::get_mut(&mut state).unwrap(), outputs);
    }
}

fn run_locally<T: IsTest>(test: T) {
    let mut pool = LocalPool::new();
    let spawner = pool.spawner();
    let fut = run_with_spawner(test, &spawner);
    pool.run();
    block_on(fut);
}

fn run_threaded<T: IsTest>(test: T) {
    let mut pool = ThreadPool::new().unwrap();
    let fut = run_with_spawner(test, &pool);
    block_on(fut);
}

#[test]
fn test_simple_local() {
    run_locally(simple_test(100))
}

#[test]
fn test_simple_threaded() {
    run_threaded(simple_test(100));
}


#[test]
fn test_lock1() {
    let (test_waker, waker) = TestWaker::new();
    let mut cx = Context::from_waker(&waker);
    assert_eq!((1, 0), test_waker.load());
    let mutex = Mutex::new(1usize);
    {
        let fut = async {
            let mut guard = mutex.lock().await;
            ready(()).await;
            *guard += 1;
            *guard
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
    let mut cx1 = Context::from_waker(&waker1);
    let mut cx2 = Context::from_waker(&waker2);
    assert_eq!((1, 0), test_waker1.load());
    assert_eq!((1, 0), test_waker2.load());
    let mutex = Mutex::new(1usize);
    {
        let fut1 = mutex.lock();
        let fut2 = mutex.lock();
        pin_mut!(fut1);
        pin_mut!(fut2);

        let guard1 = fut1.poll(&mut cx1);
        assert!(guard1.is_ready());
        assert_eq!((1, 0), test_waker1.load());
        assert_eq!((1, 0), test_waker2.load());

        assert!(fut2.as_mut().poll(&mut cx2).is_pending());
        assert_eq!((1, 0), test_waker1.load());
        assert_eq!((2, 0), test_waker2.load());

        mem::drop(guard1);
        assert_eq!((1, 0), test_waker1.load());
        assert_eq!((1, 1), test_waker2.load());

        let guard2 = fut2.as_mut().poll(&mut cx2);
        assert!(guard2.is_ready());
        assert_eq!((1, 0), test_waker1.load());
        assert_eq!((1, 1), test_waker2.load());
    }
    assert_eq!((1, 0), test_waker1.load());
    assert_eq!((1, 1), test_waker2.load());
}

#[test]
fn test_cancel1() {
    let (test_waker1, waker1) = TestWaker::new();
    let (test_waker2, waker2) = TestWaker::new();
    let mut cx1 = Context::from_waker(&waker1);
    let mut cx2 = Context::from_waker(&waker2);
    let mutex = Mutex::new(1usize);
    let guard;
    {
        let fut1 = mutex.lock();
        let fut2 = mutex.lock();
        pin_mut!(fut1);
        pin_mut!(fut2);

        guard = fut1.as_mut().poll(&mut cx1);
        assert!(guard.is_ready());
        assert_eq!((1, 0), test_waker1.load());
        assert_eq!((1, 0), test_waker2.load());

        assert!(fut2.as_mut().poll(&mut cx2).is_pending());
        assert_eq!((1, 0), test_waker1.load());
        assert_eq!((2, 0), test_waker2.load());
    }
    assert_eq!((1, 0), test_waker1.load());
    assert_eq!((1, 0), test_waker2.load());
}

#[test]
fn test_wake_cancel2() {
    let (test_waker1, waker1) = TestWaker::new();
    let (test_waker2, waker2) = TestWaker::new();
    let (test_waker3, waker3) = TestWaker::new();
    let mut cx1 = Context::from_waker(&waker1);
    let mut cx2 = Context::from_waker(&waker2);
    let mut cx3 = Context::from_waker(&waker3);
    let mutex = Mutex::new(1usize);
    {
        let mut fut1 = Box::pin(mutex.lock());
        let mut fut2 = Box::pin(mutex.lock());
        let mut fut3 = Box::pin(mutex.lock());

        let guard = fut1.as_mut().poll(&mut cx1);
        assert!(guard.is_ready());
        assert_eq!((1, 0), test_waker1.load());
        assert_eq!((1, 0), test_waker2.load());
        assert_eq!((1, 0), test_waker3.load());

        assert!(fut2.as_mut().poll(&mut cx2).is_pending());
        assert_eq!((1, 0), test_waker1.load());
        assert_eq!((2, 0), test_waker2.load());
        assert_eq!((1, 0), test_waker3.load());

        assert!(fut3.as_mut().poll(&mut cx3).is_pending());
        assert_eq!((1, 0), test_waker1.load());
        assert_eq!((2, 0), test_waker2.load());
        assert_eq!((2, 0), test_waker3.load());

        mem::drop(guard);
        assert_eq!((1, 0), test_waker1.load());
        assert_eq!((1, 1), test_waker2.load());
        assert_eq!((2, 0), test_waker3.load());

        mem::drop(fut2);
        assert_eq!((1, 0), test_waker1.load());
        assert_eq!((1, 1), test_waker2.load());
        assert_eq!((1, 1), test_waker3.load());
    }
    assert_eq!((1, 0), test_waker1.load());
    assert_eq!((1, 1), test_waker2.load());
    assert_eq!((1, 1), test_waker3.load());
}

#[test]
fn test_cancel_locally() {
    run_locally(cancel_test(100, 100))
}

#[test]
fn test_cancel_threaded() {
    run_threaded(cancel_test(100000, 100000))
}