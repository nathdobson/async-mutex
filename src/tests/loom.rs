use crate::tests::test::{IsTest, simple_test, cancel_test};
use crate::sync::Arc;
use crate::thread;
use crate::loom::model::Builder;
use std::path::{PathBuf, Path};

fn run_loom<T: IsTest>(mut builder: Builder, test: T) {
    //builder.checkpoint_file = Some(Path::new("checkpoints").join(std::thread::current().name().unwrap()));
    builder.check(move || {
        let mut state = Arc::new(test.start());
        let mut tasks: Vec<_> = (0..test.tasks()).map(|task| {
            let test = test.clone();
            let state = state.clone();
            move || crate::loom::future::block_on(test.run(state, task))
        }).collect();
        let mut tasks = tasks.into_iter();
        let main = tasks.next().unwrap();
        let threads: Vec<_> = tasks.map(thread::spawn).collect();
        let mut results = vec![];
        results.push(main());
        for thread in threads.into_iter() {
            results.push(thread.join().unwrap());
        }
        test.stop(Arc::get_mut(&mut state).unwrap(), results);
    });
}

#[test]
fn test_simple_loom_1_INF() {
    let mut builder = Builder::new();
    builder.preemption_bound = None;
    run_loom(builder, simple_test(1));
}

#[test]
fn test_simple_loom_2_INF() {
    let mut builder = Builder::new();
    builder.preemption_bound = None;
    run_loom(builder, simple_test(2));
}

#[test]
fn test_simple_loom_3_3() {
    let mut builder = Builder::new();
    builder.preemption_bound = Some(3);
    run_loom(builder, simple_test(3));
}

#[test]
fn test_simple_loom_3_4() {
    let mut builder = Builder::new();
    builder.preemption_bound = Some(4);
    run_loom(builder, simple_test(3));
}

#[test]
fn test_simple_loom_4_2() {
    let mut builder = Builder::new();
    builder.preemption_bound = Some(2);
    run_loom(builder, simple_test(4));
}

#[test]
fn test_cancel_loom_1_1_INF() {
    let mut builder = Builder::new();
    builder.preemption_bound = None;
    run_loom(builder, cancel_test(1, 1));
}

#[test]
fn test_cancel_loom_1_2_3() {
    let mut builder = Builder::new();
    builder.preemption_bound = Some(3);
    run_loom(builder, cancel_test(1, 2));
}

#[test]
fn test_cancel_loom_2_1_3() {
    let mut builder = Builder::new();
    builder.preemption_bound = Some(3);
    run_loom(builder, cancel_test(2, 1));
}

#[test]
fn test_cancel_loom_1_2_4() {
    let mut builder = Builder::new();
    builder.preemption_bound = Some(4);
    run_loom(builder, cancel_test(1, 2));
}

#[test]
fn test_cancel_loom_2_1_4() {
    let mut builder = Builder::new();
    builder.preemption_bound = Some(4);
    run_loom(builder, cancel_test(2, 1));
}
