#![allow(non_camel_case_types, unused_macros)]

macro_rules! async_traits {
    () => {};
    (
        pub trait $trait:ident $(<$($trait_arg:ident $(: $bound1:tt $(PLUS $bounds:tt)* )? ),*>)? $(: $trait_bound1:tt $(PLUS $trait_bounds:tt)* )?{
            name $mod:ident;
            async fn $method:ident<'a>(&'a $(mut $($MUT:lifetime)*)? self $(,$arg:ident: $fun_arg:ty)*)
            $(-> $type_output:tt)?
            $(IMPL -> impl $impl_output:tt)?
            ;
        }
        $($remainder:tt)*
    ) => {
        pub mod $mod {
            use crate::future::Future;
            use super::*;

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
                Copy + Clone + Send + Sync + 'static + for<'a> MethodInst<'a, S $(,$($trait_arg),*)?> {}

            impl<S $(,$($trait_arg $(: $bound1 $(+ $bounds)+ )? ),*)?, T>
                Method<S $(,$($trait_arg),*)?> for T
                where T: Copy + Clone + Send + Sync + 'static + for<'a> MethodInst<'a, S $(,$($trait_arg),*)?>
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

            pub trait $trait $(<$($trait_arg $(: $bound1 $(+ $bounds)+ )? ),*>)?: Sized $( + $trait_bound1 $(+ $trait_bounds)* )?{
                type Inner;
                type FnImpl: Method<Self::Inner $(,$($trait_arg),*)?>;
                fn $method<'a>(
                    &'a $(mut $($MUT)*)? self,
                    $($arg: $fun_arg),*
                ) -> TraitFuture<'a, Self $(,$($trait_arg),*)?>;
            }

            pub struct TraitWrapper<S, F>(pub S, pub F);

            impl<
                    S $(: $trait_bound1 $(+ $trait_bounds)* )?
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
        async_traits!{$($remainder)*}
    };

    (
        impl$(<$($impl_arg:ident $(: $bound1:tt $(PLUS $bounds:tt)* )? ),*>)? $trait:ident$(<$($trait_arg:ident),*>)?  for $struct:ty as $impl_name:ident{
            name $mod:ident;
            async fn $method:ident = $impl:expr;
        }
        $($remainder:tt)*
    ) => {
        pub fn $impl_name $(<$($impl_arg $(: $bound1 $(+ $bounds)* )? ),*>)? (this: $struct) -> impl $mod::$trait $(<$($trait_arg),*>)? {
            $mod::TraitWrapper(this, $impl)
        }
        async_traits!{$($remainder)*}
    };
}
