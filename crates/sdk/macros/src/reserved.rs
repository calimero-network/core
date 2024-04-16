use std::rc::Rc;

pub mod idents {
    use super::*;

    thread_local! {
        static CALIMERO_INPUT_IDENT: Rc<syn::Ident> = Rc::new(syn::Ident::new("CALIMERO_INPUT", proc_macro2::Span::call_site()));
    }

    pub fn input() -> ReservedRef<syn::Ident> {
        CALIMERO_INPUT_IDENT.with(|ident| ReservedRef {
            inner: ident.clone(),
        })
    }
}

pub mod lifetimes {
    use super::*;

    thread_local! {
        static CALIMERO_INPUT_LIFETIME: Rc<syn::Lifetime> = Rc::new(syn::Lifetime::new("'CALIMERO_INPUT", proc_macro2::Span::call_site()));
    }

    pub fn input() -> ReservedRef<syn::Lifetime> {
        CALIMERO_INPUT_LIFETIME.with(|lifetime| ReservedRef {
            inner: lifetime.clone(),
        })
    }
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
