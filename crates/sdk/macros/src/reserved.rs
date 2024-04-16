use std::rc::Rc;

pub mod idents {
    super::lazy! {
        input: syn::Ident = syn::Ident::new("CALIMERO_INPUT", proc_macro2::Span::call_site()),
    }
}

pub mod lifetimes {
    super::lazy! {
        input: syn::Lifetime = syn::Lifetime::new("'CALIMERO_INPUT", proc_macro2::Span::call_site()),
    }
}

pub fn init() {
    idents::init();
    lifetimes::init();
}

pub struct ReservedRef<T> {
    inner: Rc<T>,
}

impl<T> std::ops::Deref for ReservedRef<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl<T> AsRef<T> for ReservedRef<T> {
    fn as_ref(&self) -> &T {
        &self.inner
    }
}

impl<T: Clone> ReservedRef<T> {
    pub fn to_owned(&self) -> T {
        (*self.inner).clone()
    }
}

impl<T: quote::ToTokens> quote::ToTokens for ReservedRef<T> {
    fn to_tokens(&self, tokens: &mut proc_macro2::TokenStream) {
        self.inner.to_tokens(tokens)
    }
}

macro_rules! _lazy {
    ($($name:ident: $ty:ty = $init:expr,)*) => {
        use std::cell::RefCell;
        use std::rc::Rc;

        mod locals {
            use super::*;

            thread_local! {
                $(
                    #[allow(non_upper_case_globals)]
                    pub static $name: RefCell<Rc<$ty>> = panic!("uninitialized lazy item");
                )*
            }
        }

        struct LazyInit;

        impl LazyInit {
            $(
                fn $name() {
                    locals::$name.set(Rc::new($init));
                }
            )*
        }

        pub fn init() {
            $( LazyInit::$name(); )*
        }

        $(
            #[track_caller]
            pub fn $name() -> super::ReservedRef<$ty> {
                super::ReservedRef {
                    inner: locals::$name.with(|item| item.borrow().clone())
                }
            }
        )*
    };
}

use _lazy as lazy;
