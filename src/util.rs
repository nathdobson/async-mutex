use crate::future::Future;
use std::marker::PhantomData;
use std::task::{Context, Poll};
use std::pin::Pin;
use std::mem;
use std::backtrace::{Backtrace, BacktraceStatus};
use std::process::abort;

// pub trait AsyncFnOnce<Args>: FnOnce<Args, Output=Self::Fut> {
//     type Fut: Future<Output=Self::AsyncOutput>;
//     type AsyncOutput;
//     fn call_async(self, args: Args) -> Self::Fut;
// }
//
// impl<F, Args> AsyncFnOnce<Args> for F where F: FnOnce<Args>, <Self as FnOnce<Args>>::Output: Future {
//     type Fut = <Self as FnOnce<Args>>::Output;
//     type AsyncOutput = <<Self as FnOnce<Args>>::Output as Future>::Output;
//
//     fn call_async(self, args: Args) -> Self::Fut {
//         unimplemented!()
//     }
// }
//
// pub struct Curried<A, F> { phantom: PhantomData<A>, fun: F }
//
// impl<A, F> FnOnce<A> for Curried<A, F> {
//     type Output = Bind<A, F>;
//     extern "rust-call" fn call_once(self, args: A) -> Self::Output {
//         Bind { args: args, fun: self.fun }
//     }
// }
//
// pub struct Bind<A, F> { args: A, fun: F }
//
// impl<A, B, F> FnOnce<B> for Bind<A, F> where F: FnOnce<A::Joined>, A: Join<B> {
//     type Output = F::Output;
//     extern "rust-call" fn call_once(self, args: B) -> Self::Output {
//         self.fun.call_once(self.args.join(args))
//     }
// }
//
// pub trait FnOnceExt<Args>: FnOnce<Args> + Sized {
//     fn curry<A>(self) -> Curried<A, Self> {
//         Curried { phantom: PhantomData, fun: self }
//     }
// }
//
// impl<Args, F> FnOnceExt<Args> for F where F: FnOnce<Args> + Sized {}
//
// pub trait Join<B> {
//     type Joined;
//     fn join(self, y: B) -> Self::Joined;
// }
//
// impl<R> Join<()> for R {
//     type Joined = R;
//     fn join(self, other: ()) -> Self::Joined { self }
// }
//
// impl<T1> Join<(T1, )> for () {
//     type Joined = (T1, );
//     fn join(self, (t1, ): (T1, )) -> Self::Joined {
//         (t1, )
//     }
// }
//
// impl<T1, T2> Join<(T1, T2)> for () {
//     type Joined = (T1, T2);
//     fn join(self, (t1, t2): (T1, T2)) -> Self::Joined {
//         (t1, t2)
//     }
// }
//
// impl<T1, T2> Join<(T2, )> for (T1, ) {
//     type Joined = (T1, T2);
//     fn join(self, (t2, ): (T2, )) -> Self::Joined {
//         let (t1, ) = self;
//         (t1, t2)
//     }
// }
//
// impl<T1, T2, T3> Join<(T1, T2, T3)> for () {
//     type Joined = (T1, T2, T3);
//     fn join(self, (t1, t2, t3): (T1, T2, T3)) -> Self::Joined {
//         (t1, t2, t3)
//     }
// }
//
// impl<T1, T2, T3> Join<(T2, T3)> for (T1, ) {
//     type Joined = (T1, T2, T3);
//     fn join(self, (t2, t3): (T2, T3)) -> Self::Joined {
//         let (t1, ) = self;
//         (t1, t2, t3)
//     }
// }
//
// impl<T1, T2, T3> Join<(T3, )> for (T1, T2) {
//     type Joined = (T1, T2, T3);
//     fn join(self, (t3, ): (T3, )) -> Self::Joined {
//         let (t1, t2) = self;
//         (t1, t2, t3)
//     }
// }
//
// impl<T1, T2, T3, T4> Join<(T1, T2, T3, T4)> for () {
//     type Joined = (T1, T2, T3, T4);
//     fn join(self, (t1, t2, t3, t4): (T1, T2, T3, T4)) -> Self::Joined {
//         (t1, t2, t3, t4)
//     }
// }
//
// impl<T1, T2, T3, T4> Join<(T2, T3, T4)> for (T1, ) {
//     type Joined = (T1, T2, T3, T4);
//     fn join(self, (t2, t3, t4): (T2, T3, T4)) -> Self::Joined {
//         let (t1, ) = self;
//         (t1, t2, t3, t4)
//     }
// }
//
// impl<T1, T2, T3, T4> Join<(T3, T4)> for (T1, T2) {
//     type Joined = (T1, T2, T3, T4);
//     fn join(self, (t3, t4): (T3, T4)) -> Self::Joined {
//         let (t1, t2) = self;
//         (t1, t2, t3, t4)
//     }
// }
//
// impl<T1, T2, T3, T4> Join<(T4, )> for (T1, T2, T3) {
//     type Joined = (T1, T2, T3, T4);
//     fn join(self, (t4, ): (T4, )) -> Self::Joined {
//         let (t1, t2, t3) = self;
//         (t1, t2, t3, t4)
//     }
// }
//
// fn test_curry() {
//     fn add(a: i8, b: i16, c: i32, d: i64) -> i128 { a as i128 + b as i128 + c as i128 + d as i128 }
//     (add).curry()(1, 2)(3, 4);
// }

pub fn yield_now() -> impl Future {
    struct YieldNow(bool);
    impl Unpin for YieldNow {}
    impl Future for YieldNow {
        type Output = ();
        fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
            if mem::replace(&mut self.get_mut().0, true) {
                Poll::Ready(())
            } else {
                cx.waker().wake_by_ref();
                Poll::Pending
            }
        }
    }
    YieldNow(false)
}

macro_rules! test_println {
    ($($xs:tt)*) => {
        {
            //println!($($xs)*);
        }
    }
}