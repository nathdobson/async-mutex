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
use crate::cancel::Cancel;
use crate::util::yield_now;
use futures::pin_mut;
use crate::test_waker::TestWaker;

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
        let cancel = Cancel::new();
        let fut = async {
            let scope = mutex.scope();
            scope.with(async {
                let mut guard = scope.lock(&cancel).await;
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
    let mut cx1 = Context::from_waker(&waker1);
    let mut cx2 = Context::from_waker(&waker2);
    assert_eq!((1, 0), test_waker1.load());
    assert_eq!((1, 0), test_waker2.load());
    let mutex = Mutex::new(1usize);
    let cancel = Cancel::new();
    {
        async fn add_fetch(mutex: &Mutex<usize>, cancel: &Cancel) -> usize {
            let scope = mutex.scope();
            scope.with(async {
                let mut guard = scope.lock(cancel).await;
                *guard += 1;
                yield_now().await;
                *guard
            }).await
        }
        let fut1 = add_fetch(&mutex, &cancel);
        let fut2 = add_fetch(&mutex, &cancel);
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
fn test_cancel() {
    let (test_waker1, waker1) = TestWaker::new();
    let (test_waker2, waker2) = TestWaker::new();
    let mut cx1 = Context::from_waker(&waker1);
    let mut cx2 = Context::from_waker(&waker2);
    assert_eq!((1, 0), test_waker1.load());
    assert_eq!((1, 0), test_waker2.load());
    let mutex = Mutex::new(1usize);
    let cancel1 = Cancel::new();
    let cancel2 = Cancel::new();
    {
        let fut1 = async {
            let scope = mutex.scope();
            scope.with(async {
                let mut guard = scope.lock(&cancel1).await;
                yield_now().await;
                mem::drop(guard);
            }).await;
        };
        let fut2 = async {
            let scope = mutex.scope();
            #[allow(unreachable_code)]
                scope.with(async {
                let mut guard = scope.lock(&cancel2).await;
                unreachable!();
                mem::drop(guard);
            }).await;
        };
        pin_mut!(fut1);
        pin_mut!(fut2);

        assert_eq!(Poll::Pending, fut1.as_mut().poll(&mut cx1));
        assert_eq!((2, 1), test_waker1.load());
        assert_eq!((1, 0), test_waker2.load());

        assert_eq!(Poll::Pending, fut2.as_mut().poll(&mut cx2));
        assert_eq!((2, 1), test_waker1.load());
        assert_eq!((2, 0), test_waker2.load());

        cancel2.set_cancelling();
        cancel2.clear_pending();
        assert_eq!(Poll::Pending, fut2.as_mut().poll(&mut cx2));
        assert_eq!((1, 2), test_waker1.load());
        assert_eq!((2, 0), test_waker2.load());
        assert!(cancel2.pending());

        assert_eq!(Poll::Ready(()), fut1.as_mut().poll(&mut cx1));
        assert_eq!((1, 2), test_waker1.load());
        assert_eq!((1, 1), test_waker2.load());

        cancel2.clear_pending();
        assert_eq!(Poll::Pending, fut2.as_mut().poll(&mut cx2));
        assert_eq!((1, 2), test_waker1.load());
        assert_eq!((1, 1), test_waker2.load());
        assert!(!cancel2.pending());
    }
    assert_eq!((1, 2), test_waker1.load());
    assert_eq!((1, 1), test_waker2.load());
}

#[test]
fn test_cancel_locally() {
    run_locally(cancel_test(100, 100))
}

#[test]
fn test_cancel_threaded() {
    run_threaded(cancel_test(1000000, 1000000))
}