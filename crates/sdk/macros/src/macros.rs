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
