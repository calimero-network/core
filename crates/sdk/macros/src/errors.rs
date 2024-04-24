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

static TAG: &str = "(calimero)>";

#[derive(Debug, Error)]
pub enum ParseError<'a> {
    #[error("{TAG} trait impls are not supported")]
    NoTraitSupport,
    #[error("{TAG} cannot ascribe app logic to primitive types")]
    UnsupportedImplType,
    #[error("{TAG} expected `Self` or `{0}`")]
    ExpectedSelf(Pretty<'a>),
    #[error("{TAG} exposing an async method is not supported")]
    NoAsyncSupport,
    #[error("{TAG} exposing an unsafe method is not supported")]
    NoUnsafeSupport,
    // todo! disable with `#[app::destroy]`
    #[error("{TAG} `self` must be passed by reference")]
    NoSelfOwnership,
    #[error("{TAG} fatal error: type sanitization failed, please report this issue")]
    SanitizationFailed,
    #[error("{TAG} expected an identifier, found a pattern")]
    ExpectedIdent,
    #[error("{TAG} generic types are not supported")]
    NoGenericTypeSupport,
    #[error("{TAG} state lifetimes are not supported")]
    NoGenericLifetimeSupport,
    #[error("{TAG} this lifetime is reserved")]
    UseOfReservedLifetime,
    #[error("{TAG} this identifier is reserved")]
    UseOfReservedIdent,
    #[error("{TAG} this lifetime has not been declared{append}")]
    UseOfUndeclaredLifetime { append: String },
    #[error("{TAG} this lifetime must be specified")]
    MustSpecifyLifetime,
    #[error("{TAG} this event must be public")]
    NoPrivateEvent,
    #[error("{TAG} please use a simple `pub` directive")]
    NoComplexVisibility,
    #[error("{TAG} explicit ABIs are not supported")]
    NoExplicitAbi,
    #[error("{TAG} {0}")]
    Custom(&'a str),
}

impl<'a> AsRef<ParseError<'a>> for ParseError<'a> {
    fn as_ref(&self) -> &ParseError<'a> {
        self
    }
}

#[derive(Debug)]
pub struct ErrorsInner<'a, T> {
    item: &'a T,
    errors: Option<syn::Error>,
}

#[derive(Debug)]
pub struct Errors<'a, T = ()> {
    inner: RefCell<Option<ErrorsInner<'a, T>>>,
}

impl<'a> Default for Errors<'a> {
    fn default() -> Self {
        Self::new(&())
    }
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

pub enum MaybeError {
    None,
    Some(syn::Error),
}

impl From<MaybeError> for Option<syn::Error> {
    fn from(error: MaybeError) -> Self {
        match error {
            MaybeError::None => None,
            MaybeError::Some(error) => Some(error),
        }
    }
}

impl<T: Into<syn::Error>> From<T> for MaybeError {
    fn from(error: T) -> Self {
        MaybeError::Some(error.into())
    }
}

impl<'a, T> From<Errors<'a, T>> for MaybeError {
    fn from(errors: Errors<'a, T>) -> Self {
        let inner = errors.inner();

        inner.errors.map_or(MaybeError::None, MaybeError::Some)
    }
}

impl<'a, T> Errors<'a, T> {
    fn push_error(&mut self, error: syn::Error) {
        match &mut self.inner_mut().errors {
            err @ None => *err = Some(error),
            Some(err) => {
                err.combine(error);
            }
        }
    }

    pub fn push<'e, E: AsRef<ParseError<'e>>>(&mut self, span: proc_macro2::Span, error: E) {
        self.push_error(syn::Error::new(span, error.as_ref()));
    }

    pub fn push_spanned<'e, U: ToTokens, E: AsRef<ParseError<'e>>>(
        &mut self,
        tokens: &U,
        error: E,
    ) {
        self.push_error(syn::Error::new_spanned(tokens, error.as_ref()));
    }

    pub fn finish<'e, U: ToTokens, E: AsRef<ParseError<'e>>>(
        mut self,
        tokens: &U,
        error: E,
    ) -> Self {
        self.push_spanned(tokens, error);

        self
    }

    pub fn subsume<E: Into<MaybeError>>(self, other: E) -> Self {
        match other.into() {
            MaybeError::None => {}
            MaybeError::Some(other) => {
                let mut inner = self.inner_mut();
                match &mut inner.errors {
                    err @ None => *err = Some(other),
                    Some(err) => err.combine(other),
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
