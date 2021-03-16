use crate::{thread, Mutex};
use crate::sync::Arc;
use crate::tests::test::{IsTest, simple_test, cancel_test, condvar_test, channel_test, rwlock_test};
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
use crate::tests::test_waker::{TestWaker, TestPool};
use crate::condvar::Condvar;
use crate::mpsc::channel;
use Poll::Ready;
use Poll::Pending;

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
    run_threaded(simple_test(10000));
}

//
// #[test]
// fn test_lock1() {
//     let (test_waker, waker) = TestWaker::new();
//     let mut cx = Context::from_waker(&waker);
//     assert_eq!((1, 0), test_waker.load());
//     let mutex = Mutex::new(1usize);
//     {
//         let fut = async {
//             let mut guard = mutex.lock().await;
//             ready(()).await;
//             *guard += 1;
//             *guard
//         };
//         pin_mut!(fut);
//         assert_eq!(Poll::Ready(2), fut.poll(&mut cx));
//     }
//     assert_eq!((1, 0), test_waker.load());
// }

#[test]
fn test_lock1() {
    let mut pool = TestPool::new();
    let mutex = Mutex::new(1);
    let mut fut1 = pool.spawn(async {
        let mut guard = mutex.lock().await;
        ready(()).await;
        *guard += 1;
        *guard
    });
    assert_eq!((Poll::Ready(2), vec![]), pool.step(&mut fut1));
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
fn test_condvar_notify_noop() {
    let (test_waker1, waker1) = TestWaker::new();
    let mut cx1 = Context::from_waker(&waker1);
    let mutex = Mutex::new(1usize);
    let condvar = Condvar::new();
    {
        let mut fut1 = Box::pin(async {
            let mut lock = mutex.lock().await;
            condvar.notify(&mut lock, 1);
        });
        pin_mut!(fut1);
        assert_eq!(Poll::Ready(()), fut1.as_mut().poll(&mut cx1));
        assert_eq!((1, 0), test_waker1.load());
    }
    assert_eq!((1, 0), test_waker1.load());
}

#[test]
fn test_condvar_notify_once() {
    let (test_waker1, waker1) = TestWaker::new();
    let (test_waker2, waker2) = TestWaker::new();
    let mut cx1 = Context::from_waker(&waker1);
    let mut cx2 = Context::from_waker(&waker2);
    let mutex = Mutex::new(1usize);
    let condvar = Condvar::new();
    {
        let mut fut1 = Box::pin(async {
            let mut lock = mutex.lock().await;
            lock = condvar.wait(lock).await;
            assert_eq!(*lock, 2);
        });
        let mut fut2 = Box::pin(async {
            let mut lock = mutex.lock().await;
            condvar.notify(&mut lock, 1);
            *lock = 2;
            mem::drop(lock);
        });
        pin_mut!(fut1, fut2);

        assert_eq!(Poll::Pending, fut1.as_mut().poll(&mut cx1));
        assert_eq!((2, 0), test_waker1.load());
        assert_eq!((1, 0), test_waker2.load());

        assert_eq!(Poll::Ready(()), fut2.as_mut().poll(&mut cx2));
        assert_eq!((1, 1), test_waker1.load());
        assert_eq!((1, 0), test_waker2.load());

        assert_eq!(Poll::Ready(()), fut1.as_mut().poll(&mut cx1));
        assert_eq!((1, 1), test_waker1.load());
        assert_eq!((1, 0), test_waker2.load());
    }
    assert_eq!((1, 1), test_waker1.load());
    assert_eq!((1, 0), test_waker2.load());
}

#[test]
fn test_channel_send_recv_1() {
    let mut pool = TestPool::new();
    let (sender, mut receiver) = channel(1);
    let mut fut1 = pool.spawn(sender.send(1));
    let mut fut2 = pool.spawn(receiver.recv());
    assert_eq!((Ready(Ok(())), vec![]), pool.step(&mut fut1));
    assert_eq!((Ready(Ok(1)), vec![]), pool.step(&mut fut2));
}

#[test]
fn test_channel_recv_send_1() {
    let mut pool = TestPool::new();
    let (sender, mut receiver) = channel(1);
    let mut fut0 = pool.spawn(receiver.recv());
    let mut fut1 = pool.spawn(sender.send(1));
    assert_eq!((Pending, vec![]), pool.step(&mut fut0));
    assert_eq!((Ready(Ok(())), vec![0]), pool.step(&mut fut1));
    assert_eq!((Ready(Ok(1)), vec![]), pool.step(&mut fut0));
    println!("{:?}", sender);
}

#[test]
fn test_channel_recv_send_2() {
    let mut pool = TestPool::new();
    let (sender, mut receiver) = channel(1);
    let mut fut0 = pool.spawn(async {
        assert_eq!(Ok(1), receiver.recv().await);
        assert_eq!(Ok(2), receiver.recv().await);
    });
    let mut fut1 = pool.spawn(async {
        sender.send(1).await.unwrap();
        sender.send(2).await.unwrap();
    });
    assert_eq!((Pending, vec![]), pool.step(&mut fut0));
    assert_eq!((Pending, vec![0]), pool.step(&mut fut1));
    assert_eq!((Ready(()), vec![1]), pool.step(&mut fut0));
    assert_eq!((Ready(()), vec![]), pool.step(&mut fut1));
}

#[test]
fn test_cancel_locally() {
    run_locally(cancel_test(100, 100))
}

#[test]
fn test_cancel_threaded() {
    run_threaded(cancel_test(100000, 100000))
}

#[test]
fn test_condvar_locally() {
    run_locally(condvar_test(1))
}

#[test]
fn test_condvar_threaded() {
    run_threaded(condvar_test(10000))
}

#[test]
fn test_channel_locally() {
    run_locally(channel_test(&[2], 1))
}

#[test]
fn test_channel_threaded() {
    run_threaded(channel_test(&[10000; 10], 100))
}

#[test]
fn test_rwlock_locally() {
    run_locally(rwlock_test(1, 1))
}

#[test]
fn test_rwlock_threaded() {
    run_threaded(rwlock_test(100000,0))
}