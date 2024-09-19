use core::ops::Deref;
use std::rc::Rc;

use proc_macro2::{Span, TokenStream};
use quote::ToTokens;
use syn::{Ident, Lifetime};

pub mod idents {
    use super::{lazy, Ident, Span};
    lazy! {Ident => {
        input = Ident::new("CalimeroInput", Span::call_site()),
    }}
}

pub mod lifetimes {
    use super::{lazy, Lifetime, Span};
    lazy! {Lifetime => {
        input = Lifetime::new("'CALIMERO_INPUT", Span::call_site()),
    }}
}

pub fn init() {
    idents::init();
    lifetimes::init();
}

pub struct ReservedRef<T> {
    inner: Rc<T>,
}

impl<T> Deref for ReservedRef<T> {
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

impl<T: ToTokens> ToTokens for ReservedRef<T> {
    fn to_tokens(&self, tokens: &mut TokenStream) {
        self.inner.to_tokens(tokens);
    }
}

macro_rules! _lazy {
    ($ty:ty => {$($name:ident = $init:expr,)*}) => {
        use core::cell::RefCell;
        use std::rc::Rc;

        mod locals {
            use super::*;

            thread_local! {
                $(
                    #[expect(non_upper_case_globals)]
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
