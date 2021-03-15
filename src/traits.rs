#![allow(non_camel_case_types)]

macro_rules! async_traits {
    (
        pub trait $trait:ident $(<$($trait_arg:ident $(: $bound1:tt $(PLUS $bounds:tt)* )? ),*>)? {
            name $mod:ident;
            async fn $method:ident<'a>(&'a $(mut $($MUT:lifetime)*)? self $(,$arg:ident: $fun_arg:ty)*)
            $(-> $type_output:tt)?
            $(IMPL -> impl $impl_output:tt)?
            ;
        }
    ) => {
        pub mod $mod {
            use crate::future::Future;

            pub trait MethodInst<'a, S: 'a $(,$($trait_arg $(: $bound1 $(+ $bounds)+ )? ),*)?> {
                $(
                    type AsyncOutput: $impl_output;
                    type Fut: Future<Output=Self::AsyncOutput> + Send;
                )?
                $(
                    type Fut: Future<Output=$type_output> + Send;
                )?
                fn call(&self, s: &'a $(mut $($MUT)*)? S, $($arg: $fun_arg),*) -> Self::Fut;
            }

            pub trait Pair{type First;type Second;}
            impl<A,B> Pair for (A,B){
                type First=A;
                type Second=B;
            }

            impl<'a, S: 'a $(,$($trait_arg $(: $bound1 $(+ $bounds)+ )? ),*)?, T>
                MethodInst<'a, S $(,$($trait_arg),*)?> for T
                where
                    T: Fn<
                        (&'a $(mut $($MUT)*)? S, $($fun_arg),*),
                        Output: Send + Future<Output
                            $(
                                = $type_output
                            )?
                            $(
                                : $impl_output
                            )?
                        >
                    >
                {
                $(
                    type AsyncOutput = <(<T::Output as Future>::Output, &'static dyn $impl_output) as Pair>::First;
                )?
                type Fut = T::Output;
                fn call(&self, s: &'a $(mut $($MUT)*)? S, $($arg: $fun_arg),*) -> Self::Fut { (self)(s, $($arg),*) }
            }

            pub trait Method<S $(,$($trait_arg $(: $bound1 $(+ $bounds)+ )? ),*)?>:
                Copy + Clone + Send + Sync + for<'a> MethodInst<'a, S $(,$($trait_arg),*)?> {}

            impl<S $(,$($trait_arg $(: $bound1 $(+ $bounds)+ )? ),*)?, T>
                Method<S $(,$($trait_arg),*)?> for T
                where T: Copy + Clone + Send + Sync + for<'a> MethodInst<'a, S $(,$($trait_arg),*)?>
                {}

            pub type MethodFuture<'a, S, F $(,$($trait_arg $(: $bound1 $(+ $bounds)+ )? ),*)?>
                = <F as MethodInst<'a, S $(,$($trait_arg),*)?>>::Fut;
            pub type MethodOutput<'a, S, F $(,$($trait_arg $(: $bound1 $(+ $bounds)+ )? ),*)?>
                = <MethodFuture<'a, F, S $(,$($trait_arg),*)?> as Future>::Output;

            pub type TraitFuture<'a, W $(,$($trait_arg $(: $bound1 $(+ $bounds)+ )? ),*)?>
                = MethodFuture<'a,
                    <W as $trait $(<$($trait_arg),*>)?>::Inner,
                    <W as $trait $(<$($trait_arg),*>)?>::FnImpl $(,$($trait_arg),*)?
                  >;

            pub trait $trait $(<$($trait_arg $(: $bound1 $(+ $bounds)+ )? ),*>)?: Sized {
                type Inner;
                type FnImpl: Method<Self::Inner $(,$($trait_arg),*)?>;
                fn $method<'a>(
                    &'a $(mut $($MUT)*)? self,
                    $($arg: $fun_arg),*
                ) -> TraitFuture<'a, Self $(,$($trait_arg),*)?>;
            }

            pub struct TraitWrapper<S, F>(pub S, pub F);

            impl<
                    S
                    $(,$($trait_arg $(: $bound1 $(+ $bounds)+ )? ),*)?,
                    FnImpl: Method<S $(,$($trait_arg),*)?>
                >
                    $trait $(<$($trait_arg),*>)?
                    for TraitWrapper<S, FnImpl> {
                type Inner = S;
                type FnImpl = FnImpl;
                fn $method<'a>(
                    &'a $(mut $($MUT)*)? self,
                    $($arg: $fun_arg),*
                ) -> TraitFuture<'a, Self $(,$($trait_arg),*)?> {
                    self.1.call(& $(mut $($MUT)*)? self.0, $($arg),*)
                }
            }
        }
        pub use $mod::$trait;
    };

    (
        impl$(<$($impl_arg:ident $(: $bound1:tt $(PLUS $bounds:tt)* )? ),*>)? $trait:ident$(<$($trait_arg:ident),*>)?  for $struct:ty as $impl_name:ident{
            name $mod:ident;
            async fn $method:ident = $impl:expr;
        }
    ) => {
        pub fn $impl_name $(<$($impl_arg $(: $bound1 $(+ $bounds)* )? ),*>)? (this: $struct) -> impl $mod::$trait $(<$($trait_arg),*>)? {
            $mod::TraitWrapper(this, $impl)
        }
    };
}

async_traits! {
    pub trait SenderTrait<M> {
        name sender_trait;
        async fn send<'a>(&'a self, msg:M) -> ();
    }
}

async_traits! {
    impl<M: Send PLUS 'static> SenderTrait<M> for crate::mpsc::Sender<M> as crate_as_sender {
        name sender_trait;
        async fn send = crate::mpsc::Sender::send;
    }
}

async_traits! {
    pub trait ReceiverTrait<M> {
        name receiver_trait;
        async fn recv<'a>(&'a mut self) -> M;
    }
}

async_traits! {
    impl<M: Send PLUS 'static> ReceiverTrait<M> for crate::mpsc::Receiver<M> as crate_as_receiver {
        name receiver_trait;
        async fn recv = crate::mpsc::Receiver::recv;
    }
}

async_traits! {
    pub trait MutexTrait<M> {
        name mutex_trait;
        async fn lock<'a>(&'a self) IMPL -> impl (std::ops::DerefMut<Target=M>);
    }
}

async_traits! {
    impl<M: Send PLUS 'static> MutexTrait<M> for crate::Mutex<M> as crate_as_mutex {
        name mutex_trait;
        async fn lock = crate::Mutex::lock;
    }
}

fn assert_send<T: Send>(x: T) -> T { x }

async fn test_sender() {
    let (s, r): (crate::mpsc::Sender<usize>, crate::mpsc::Receiver<usize>) = crate::mpsc::channel(0);
    assert_send(crate_as_sender(s).send(1)).await;
}

async fn test_receiver() {
    let (s, r): (crate::mpsc::Sender<usize>, crate::mpsc::Receiver<usize>) = crate::mpsc::channel(0);
    let x: usize = assert_send(crate_as_receiver(r).recv()).await;
    println!("{:?}", x + 1);
}

async fn test_mutex() {
    let mutex = crate::Mutex::new(1);
    let mutex = crate_as_mutex(mutex);
    let guard = assert_send(mutex.lock()).await;
    let x: usize = *guard;
}
