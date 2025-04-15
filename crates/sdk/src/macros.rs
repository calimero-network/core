#[doc(hidden)]
#[macro_export]
macro_rules! __err__ {
    ($msg:literal $(,)?) => {{
        use $crate::types::__private::*;
        (&&&&$crate::types::__private::Wrap(::std::format_args!($msg))).into_error()
    }};
    ($err:expr $(,)?) => {{
        #[allow(unused_imports, reason = "if expanding the next line fails, it reports this as unused")]
        use $crate::types::__private::*;
        (&&&&$crate::types::__private::Wrap($err)).into_error()
    }};
    ($fmt:expr, $($arg:tt)*) => {{
        use $crate::types::__private::*;
        (&&&&$crate::types::__private::Wrap(&::std::format!($fmt, $($arg)*))).into_error()
    }};
}

#[macro_export]
macro_rules! __bail__ {
    ($msg:literal $(,)?) => {
        return ::core::result::Result::Err($crate::__err__!($msg));
    };
    ($err:expr $(,)?) => {
        return ::core::result::Result::Err($crate::__err__!($err));
    };
    ($fmt:expr, $($arg:tt)*) => {
        return ::core::result::Result::Err($crate::__err__!($fmt, $($arg)*));
    };
}

#[doc(hidden)]
#[macro_export]
macro_rules! __log__ {
    ($fmt:literal $(,)?) => {
        match ::std::format_args!($fmt) {
            args => match args.as_str() {
                Some(msg) => $crate::env::log(msg),
                None => $crate::env::log(&::std::fmt::format(args)),
            },
        }
    };
    ($fmt:expr, $($arg:tt)*) => {
        $crate::env::log(&::std::format!($fmt, $($arg)*))
    };
}
