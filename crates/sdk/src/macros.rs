#[doc(hidden)]
#[macro_export]
macro_rules! __bail__ {
    ($msg:literal $(,)?) => {{
        use $crate::types::__private::*;
        return Err((&&&&$crate::types::__private::Wrap(::std::format_args!($msg))).into_error());
    }};
    ($msg:expr $(,)?) => {{
        #[allow(unused_imports, reason = "if expanding the next line fails, it reports this as unused")]
        use $crate::types::__private::*;
        return Err((&&&&$crate::types::__private::Wrap($msg)).into_error());
    }};
    ($fmt:expr, $($arg:tt)*) => {{
        use $crate::types::__private::*;
        return Err((&&&&$crate::types::__private::Wrap(&::std::format!($fmt, $($arg)*))).into_error());
    }};
}
