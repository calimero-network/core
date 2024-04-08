use std::cell::{Ref, RefCell, RefMut};
use std::fmt;

use quote::{quote, ToTokens};
use thiserror::Error;

#[derive(Debug)]
pub enum Pretty<'a> {
    Path(&'a syn::Path),
}

impl<'a> fmt::Display for Pretty<'a> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (tokens, (pre, post)) = match self {
            Self::Path(path) => (quote! { impl #path {} }, (5, 3)),
        };

        let item = syn::parse2(tokens).map_err(|err| {
            panic!("failed to parse tokens: {}", err);
        })?;

        let parsed = prettyplease::unparse(&syn::File {
            shebang: None,
            attrs: vec![],
            items: vec![item],
        });

        let parsed = parsed.trim();

        f.pad(&parsed[pre..parsed.len() - post])
    }
}

#[derive(Debug, Error)]
pub enum ParseError<'a> {
    #[error("trait impls are not supported")]
    NoTraitSupport,
    #[error("cannot ascribe app logic to primitive types")]
    UnsupportedImplType,
    #[error("expected `Self` or `{0}`")]
    ExpectedSelf(Pretty<'a>),
    #[error("async methods are not supported")]
    NoAsyncSupport,
    #[error("unsafe methods are not supported")]
    NoUnsafeSupport,
    // todo! disable with `#[app::destroy]`
    #[error("`self` must be passed by reference")]
    NoSelfOwnership,
    #[error("fatal error: self sanitization failed, please report this issue")]
    SelfSanitizationFailed,
    #[error("expected an identifier, found a pattern")]
    ExpectedIdent,
    #[error("generic types are not supported")]
    NoGenericSupport,
}

#[derive(Debug)]
pub struct ErrorsInner<'a, T> {
    item: &'a T,
    errors: Option<syn::Error>,
}

#[derive(Debug)]
pub struct Errors<'a, T> {
    inner: RefCell<Option<ErrorsInner<'a, T>>>,
}

impl<'a, T> Errors<'a, T> {
    pub fn new(item: &'a T) -> Self {
        Self {
            inner: RefCell::new(Some(ErrorsInner { item, errors: None })),
        }
    }

    fn inner(&self) -> ErrorsInner<'a, T> {
        self.inner
            .borrow_mut()
            .take()
            .expect("This instance has already been consumed")
    }

    fn inner_ref(&self) -> Ref<ErrorsInner<'a, T>> {
        Ref::map(self.inner.borrow(), |inner| {
            inner
                .as_ref()
                .expect("This instance has already been consumed")
        })
    }

    fn inner_mut(&self) -> RefMut<ErrorsInner<'a, T>> {
        RefMut::map(self.inner.borrow_mut(), |inner| {
            inner
                .as_mut()
                .expect("This instance has already been consumed")
        })
    }
}

impl<'a, T> Errors<'a, T> {
    pub fn push<U: ToTokens>(&mut self, tokens: &U, error: ParseError) {
        let error = syn::Error::new_spanned(tokens, format_args!("(calimero)> {}", error));
        match &mut self.inner_mut().errors {
            err @ None => {
                err.replace(error);
            }
            Some(err) => err.combine(error),
        };
    }

    pub fn finish<U: ToTokens>(mut self, tokens: &U, error: ParseError) -> Self {
        self.push(tokens, error);

        self
    }

    pub fn subsume<U>(self, other: Errors<'_, U>) -> Self {
        let other = other.inner();
        match &mut self.inner_mut().errors {
            err @ None => *err = other.errors,
            Some(err) => {
                if let Some(other) = other.errors {
                    err.combine(other);
                }
            }
        }
        self
    }

    pub fn check<U>(&self, val: U) -> Result<U, Self> {
        let inner = self.inner_ref().errors.is_some();
        inner.then(|| ()).map_or(Ok(val), |_| {
            Err(Errors {
                inner: RefCell::new(Some(self.inner())),
            })
        })
    }

    pub fn to_compile_error(self) -> proc_macro2::TokenStream
    where
        T: ToTokens,
    {
        let inner = self.inner();

        let mut tokens = proc_macro2::TokenStream::new();

        if let Some(err) = inner.errors {
            err.to_compile_error().to_tokens(&mut tokens);
        }

        inner.item.to_tokens(&mut tokens);

        tokens
    }
}

impl<'a, T> Drop for Errors<'a, T> {
    fn drop(&mut self) {
        if !std::thread::panicking() {
            if let Some(inner) = &*self.inner.borrow() {
                if inner.errors.is_some() {
                    panic!("forgot to check for errors");
                }
            }
        }
    }
}
