macro_rules! parse_macro_input {
    (@let $er:ident =) => { let $er = errors::Errors::default(); };
    (@let $er:ident = $item:expr) => {
        let item = proc_macro2::TokenStream::from($item);
        let $er = errors::Errors::new(&item);
    };
    ($({ $item:expr } =>)? $stream:ident as $ty:ty ) => {
        match syn::parse::<$ty>($stream) {
            Ok(data) => data,
            Err(err) => {
                $crate::macros::parse_macro_input!{@
                    let errors = $($item)?
                };
                errors.subsume(err);
                return errors.to_compile_error().into();
            }
        }
    };
}

pub(crate) use parse_macro_input;

macro_rules! infallible {
    ($body:block) => {{
        #[track_caller]
        #[inline]
        fn infallible<T, E: core::fmt::Debug, F: FnOnce() -> Result<T, E>>(f: F) -> T {
            match f() {
                Ok(value) => value,
                Err(err) => {
                    let location = core::panic::Location::caller();
                    unreachable!(
                        "infallible block failed: {:?} at {}:{}:{}",
                        err,
                        location.file(),
                        location.line(),
                        location.column()
                    );
                }
            }
        }

        infallible(
            #[inline]
            || $body,
        )
    }};
}

pub(crate) use infallible;
