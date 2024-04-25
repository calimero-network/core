macro_rules! _parse_macro_input {
    (@let $er:ident =) => { let mut $er = errors::Errors::default(); };
    (@let $er:ident = $item:expr) => {
        let item = proc_macro2::TokenStream::from($item);
        let mut $er = errors::Errors::new(&item);
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

pub(crate) use _parse_macro_input as parse_macro_input;

macro_rules! _infallible {
    ($body:block) => {{
        #[track_caller]
        #[inline(always)]
        fn infallible<T, E: std::fmt::Debug, F: FnOnce() -> Result<T, E>>(f: F) -> T {
            match f() {
                Ok(value) => value,
                Err(err) => {
                    let location = std::panic::Location::caller();
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
            #[inline(always)]
            || $body,
        )
    }};
}

pub(crate) use _infallible as infallible;
