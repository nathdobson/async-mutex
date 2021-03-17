use crate::tests::test::{IsTest, mutex_test, mutex_cancel_test, condvar_test, condvar_test_a, channel_test, rwlock_test, fast_mutex_test};
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
fn test_mutex_loom_1_inf() {
    let mut builder = Builder::new();
    builder.preemption_bound = None;
    run_loom(builder, mutex_test(1));
}

#[test]
fn test_mutex_loom_2_inf() {
    let mut builder = Builder::new();
    builder.preemption_bound = None;
    run_loom(builder, mutex_test(2));
}

#[test]
fn test_mutex_loom_3_2() {
    let mut builder = Builder::new();
    builder.preemption_bound = Some(2);
    run_loom(builder, mutex_test(3));
}

#[test]
#[cfg_attr(not(feature = "test_seconds"), ignore)]
fn test_mutex_loom_3_3() {
    let mut builder = Builder::new();
    builder.preemption_bound = Some(3);
    run_loom(builder, mutex_test(3));
}

#[test]
#[cfg_attr(not(feature = "test_seconds"), ignore)]
fn test_mutex_loom_3_4() {
    let mut builder = Builder::new();
    builder.preemption_bound = Some(4);
    run_loom(builder, mutex_test(3));
}

#[test]
fn test_mutex_loom_4_1() {
    let mut builder = Builder::new();
    builder.preemption_bound = Some(1);
    run_loom(builder, mutex_test(4));
}

#[test]
#[cfg_attr(not(feature = "test_minute"), ignore)]
fn test_mutex_loom_4_2() {
    let mut builder = Builder::new();
    builder.preemption_bound = Some(2);
    run_loom(builder, mutex_test(4));
}

#[test]
fn test_fast_mutex_loom_1_inf() {
    let mut builder = Builder::new();
    builder.preemption_bound = None;
    run_loom(builder, fast_mutex_test(1));
}

#[test]
fn test_fast_mutex_loom_2_inf() {
    let mut builder = Builder::new();
    builder.preemption_bound = None;
    run_loom(builder, fast_mutex_test(2));
}

#[test]
fn test_fast_mutex_loom_3_2() {
    let mut builder = Builder::new();
    builder.preemption_bound = Some(2);
    run_loom(builder, fast_mutex_test(3));
}

#[test]
#[cfg_attr(not(feature = "test_seconds"), ignore)]
fn test_fast_mutex_loom_3_3() {
    let mut builder = Builder::new();
    builder.preemption_bound = Some(3);
    run_loom(builder, fast_mutex_test(3));
}

#[test]
#[cfg_attr(not(feature = "test_seconds"), ignore)]
fn test_fast_mutex_loom_3_4() {
    let mut builder = Builder::new();
    builder.preemption_bound = Some(4);
    run_loom(builder, mutex_test(3));
}

#[test]
fn test_fast_mutex_loom_4_1() {
    let mut builder = Builder::new();
    builder.preemption_bound = Some(1);
    run_loom(builder, fast_mutex_test(4));
}

#[test]
#[cfg_attr(not(feature = "test_minute"), ignore)]
fn test_fast_mutex_loom_4_2() {
    let mut builder = Builder::new();
    builder.preemption_bound = Some(2);
    run_loom(builder, fast_mutex_test(4));
}

#[test]
fn test_cancel_loom_1_1_inf() {
    let mut builder = Builder::new();
    builder.preemption_bound = None;
    run_loom(builder, mutex_cancel_test(1, 1));
}


#[test]
fn test_cancel_loom_1_2_3() {
    let mut builder = Builder::new();
    builder.preemption_bound = Some(3);
    run_loom(builder, mutex_cancel_test(1, 2));
}

#[test]
#[cfg_attr(not(feature = "test_seconds"), ignore)]
fn test_cancel_loom_1_2_4() {
    let mut builder = Builder::new();
    builder.preemption_bound = Some(4);
    run_loom(builder, mutex_cancel_test(1, 2));
}

#[test]
fn test_cancel_loom_2_1_3() {
    let mut builder = Builder::new();
    builder.preemption_bound = Some(3);
    run_loom(builder, mutex_cancel_test(2, 1));
}

#[test]
#[cfg_attr(not(feature = "test_seconds"), ignore)]
fn test_cancel_loom_2_1_4() {
    let mut builder = Builder::new();
    builder.preemption_bound = Some(4);
    run_loom(builder, mutex_cancel_test(2, 1));
}

#[test]
fn test_cancel_loom_0_2_inf() {
    let mut builder = Builder::new();
    builder.preemption_bound = None;
    run_loom(builder, mutex_cancel_test(0, 2));
}

#[test]
fn test_condvar_a_2() {
    let mut builder = Builder::new();
    builder.preemption_bound = Some(2);
    run_loom(builder, condvar_test_a());
}

#[test]
#[cfg_attr(not(feature = "test_seconds"), ignore)]
fn test_condvar_a_3() {
    let mut builder = Builder::new();
    builder.preemption_bound = Some(3);
    run_loom(builder, condvar_test_a());
}

#[test]
#[cfg_attr(not(feature = "test_minute"), ignore)]
fn test_condvar_a_4() {
    let mut builder = Builder::new();
    builder.preemption_bound = Some(4);
    run_loom(builder, condvar_test_a());
}

#[test]
fn test_condvar_1_inf() {
    let mut builder = Builder::new();
    builder.preemption_bound = None;
    run_loom(builder, condvar_test(1));
}

#[test]
fn test_condvar_2_1() {
    let mut builder = Builder::new();
    builder.preemption_bound = Some(1);
    run_loom(builder, condvar_test(2));
}

#[test]
#[cfg_attr(not(feature = "test_minute"), ignore)]
fn test_condvar_2_2() {
    let mut builder = Builder::new();
    builder.preemption_bound = Some(2);
    run_loom(builder, condvar_test(2));
}


#[test]
fn test_channel_a_4() {
    let mut builder = Builder::new();
    builder.preemption_bound = Some(6);
    run_loom(builder, channel_test(&[2], 1));
}

#[test]
#[cfg_attr(not(feature = "test_seconds"), ignore)]
fn test_channel_a() {
    let mut builder = Builder::new();
    builder.preemption_bound = None;
    run_loom(builder, channel_test(&[2], 1));
}

#[test]
#[cfg_attr(not(feature = "test_seconds"), ignore)]
fn test_channel_b_3() {
    let mut builder = Builder::new();
    builder.preemption_bound = Some(4);
    run_loom(builder, channel_test(&[1, 1], 1));
}

#[test]
#[cfg_attr(not(feature = "test_seconds"), ignore)]
fn test_channel_b_4() {
    let mut builder = Builder::new();
    builder.preemption_bound = Some(4);
    run_loom(builder, channel_test(&[1, 1], 1));
}

#[test]
#[cfg_attr(not(feature = "test_minute"), ignore)]
fn test_channel_b_5() {
    let mut builder = Builder::new();
    builder.preemption_bound = Some(5);
    run_loom(builder, channel_test(&[1, 1], 1));
}


#[test]
#[cfg_attr(not(feature = "test_minute"), ignore)]
fn test_channel_c_3() {
    let mut builder = Builder::new();
    builder.preemption_bound = Some(3);
    run_loom(builder, channel_test(&[2, 1], 1));
}

#[test]
#[cfg_attr(not(feature = "test_minute"), ignore)]
fn test_channel_d_4() {
    let mut builder = Builder::new();
    builder.preemption_bound = Some(4);
    run_loom(builder, channel_test(&[2, 1], 2));
}

#[test]
fn test_rwlock_1_1_inf() {
    let mut builder = Builder::new();
    builder.preemption_bound = None;
    run_loom(builder, rwlock_test(1, 1));
}

#[test]
fn test_rwlock_2_0_inf() {
    let mut builder = Builder::new();
    builder.preemption_bound = None;
    run_loom(builder, rwlock_test(2, 0));
}

#[test]
fn test_rwlock_0_2_inf() {
    let mut builder = Builder::new();
    builder.preemption_bound = None;
    run_loom(builder, rwlock_test(0, 2));
}

#[test]
fn test_rwlock_1_2_3() {
    let mut builder = Builder::new();
    builder.preemption_bound = Some(3);
    run_loom(builder, rwlock_test(1, 2));
}

#[test]
#[cfg_attr(not(feature = "test_seconds"), ignore)]
fn test_rwlock_1_2_4() {
    let mut builder = Builder::new();
    builder.preemption_bound = Some(4);
    run_loom(builder, rwlock_test(1, 2));
}

#[test]
fn test_rwlock_2_1_3() {
    let mut builder = Builder::new();
    builder.preemption_bound = Some(3);
    run_loom(builder, rwlock_test(2, 1));
}

#[test]
#[cfg_attr(not(feature = "test_seconds"), ignore)]
fn test_rwlock_2_1_4() {
    let mut builder = Builder::new();
    builder.preemption_bound = Some(4);
    run_loom(builder, rwlock_test(2, 1));
}

