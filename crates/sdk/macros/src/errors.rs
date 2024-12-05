use core::cell::{Ref, RefCell, RefMut};
use core::fmt::{self, Display, Formatter};
use core::hint::unreachable_unchecked;
use core::panic::Location as PanicLocation;
use std::thread::panicking;

use prettyplease::unparse;
use proc_macro2::TokenStream;
use quote::{quote, quote_spanned, ToTokens};
use syn::{parse2, Error as SynError, File, Path, Type};
use thiserror::Error as ThisError;

#[derive(Debug)]
pub enum Pretty<'a> {
    Path(&'a Path),
    Type(&'a Type),
}

impl Display for Pretty<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        let (tokens, (pre, post)) = match self {
            Self::Type(ty) => (quote! { impl #ty {} }, (5, 3)),
            Self::Path(path) => (quote! { impl #path {} }, (5, 3)),
        };

        let item = parse2(tokens).map_err(|_err| fmt::Error)?;

        let parsed = unparse(&File {
            shebang: None,
            attrs: vec![],
            items: vec![item],
        });

        let parsed = parsed.trim();

        f.pad(
            parsed
                .get(pre..parsed.len().saturating_sub(post))
                .ok_or(fmt::Error)?,
        )
    }
}

static TAG: &str = "(calimero)>";

#[derive(Debug, ThisError)]
pub enum ParseError<'a> {
    #[error("trait impls are not supported")]
    NoTraitSupport,
    #[error("cannot ascribe app logic to primitive types")]
    UnsupportedImplType,
    #[error("expected `Self` or `{0}`")]
    ExpectedSelf(Pretty<'a>),
    #[error("exposing an async method is not supported")]
    NoAsyncSupport,
    #[error("exposing an unsafe method is not supported")]
    NoUnsafeSupport,
    // todo! disable with `#[app::destroy]`
    #[error("`self` must be passed by reference")]
    NoSelfOwnership,
    #[error("expected an identifier, found a pattern")]
    ExpectedIdent,
    #[error("generic types are not supported")]
    NoGenericTypeSupport,
    #[error("state lifetimes are not supported")]
    NoGenericLifetimeSupport,
    #[error("this lifetime is reserved")]
    UseOfReservedLifetime,
    #[error("this identifier is reserved")]
    UseOfReservedIdent,
    #[error("this lifetime has not been declared{append}")]
    UseOfUndeclaredLifetime { append: String },
    #[error("this lifetime must be specified")]
    MustSpecifyLifetime,
    #[error("this event must be public")]
    NoPrivateEvent,
    #[error("please use a simple `pub` directive")]
    NoComplexVisibility,
    #[error("explicit ABIs are not supported")]
    NoExplicitAbi,
    #[error("an initializer, by definition, has no `self` to reference")]
    NoSelfReceiverAtInit,
    #[error("an initializer method, by definition, has to be public")]
    NoPrivateInit,
    #[error("method named `init` must be annotated with `#[app::init]`")]
    InitMethodWithoutInitAttribute,
    #[error("method annotated with `#[app::init]` must be named `init`")]
    AppInitMethodNotNamedInit,
}

impl AsRef<Self> for ParseError<'_> {
    fn as_ref(&self) -> &Self {
        self
    }
}

#[derive(Debug)]
pub struct ErrorsInner<'a, T> {
    item: &'a T,
    errors: Option<SynError>,
    defined_at: &'static PanicLocation<'static>,
}

#[derive(Debug)]
pub struct Errors<'a, T = Void> {
    inner: RefCell<Option<ErrorsInner<'a, T>>>,
}

impl Default for Errors<'_> {
    #[track_caller]
    fn default() -> Self {
        Self::new(&Void { _priv: () })
    }
}

impl<'a, T> Errors<'a, T> {
    #[track_caller]
    pub const fn new(item: &'a T) -> Self {
        Self {
            inner: RefCell::new(Some(ErrorsInner {
                item,
                errors: None,
                defined_at: PanicLocation::caller(),
            })),
        }
    }

    fn inner(&self) -> ErrorsInner<'a, T> {
        self.inner
            .borrow_mut()
            .take()
            .expect("This instance has already been consumed")
    }

    fn inner_ref(&self) -> Ref<'_, ErrorsInner<'a, T>> {
        Ref::map(self.inner.borrow(), |inner| {
            inner
                .as_ref()
                .expect("This instance has already been consumed")
        })
    }

    fn inner_mut(&self) -> RefMut<'_, ErrorsInner<'a, T>> {
        RefMut::map(self.inner.borrow_mut(), |inner| {
            inner
                .as_mut()
                .expect("This instance has already been consumed")
        })
    }
}

impl<'a, T> Errors<'a, T> {
    pub fn subsume(&self, error: SynError) {
        match &mut self.inner_mut().errors {
            err @ None => *err = Some(error),
            Some(err) => err.combine(error),
        }
    }

    pub fn subsumed(self, other: SynError) -> SynError {
        self.subsume(other);
        let Some(errors) = self.inner().errors else {
            // safety: we know we have at least one error
            unsafe { unreachable_unchecked() }
        };
        errors
    }

    pub fn finish(self, error: SynError) -> Self {
        self.subsume(error);
        self
    }

    pub fn combine<U>(&self, other: &Errors<'a, U>) {
        if let Some(errors) = other.inner().errors {
            self.subsume(errors);
        }
    }

    pub fn check(self) -> Result<(), Self> {
        let inner = self.inner_ref().errors.is_some();
        inner.then_some(()).map_or(Ok(()), |()| Err(self))
    }

    // panics if this instance has already been consumed or "taken"
    pub fn take(&self) -> Option<SynError> {
        self.inner().errors
    }

    pub fn to_compile_error(&self) -> TokenStream
    where
        T: ToTokens,
    {
        let inner = self.inner();

        let mut tokens = TokenStream::new();

        for err in inner.errors.into_iter().flat_map(IntoIterator::into_iter) {
            let msg = err.to_string();
            quote_spanned! {err.span()=>
                ::core::compile_error!(::core::concat!(#TAG, " ", #msg));
            }
            .to_tokens(&mut tokens);
        }

        inner.item.to_tokens(&mut tokens);

        tokens
    }
}

impl<T> Drop for Errors<'_, T> {
    fn drop(&mut self) {
        if !panicking() {
            if let Some(inner) = &*self.inner.borrow() {
                assert!(
                    inner.errors.is_none(),
                    "dropped non-empty error accumulator defined at: {}:{}:{}",
                    inner.defined_at.file(),
                    inner.defined_at.line(),
                    inner.defined_at.column()
                );
            }
        }
    }
}

#[derive(Debug)]
pub struct Void {
    _priv: (),
}

impl ToTokens for Void {
    fn to_tokens(&self, _: &mut TokenStream) {}
}
