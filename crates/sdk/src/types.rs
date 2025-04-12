use std::fmt;

use serde::Serialize;
use serde_json::Value;

#[derive(Debug, Serialize)]
#[serde(transparent)]
pub struct Error {
    error: Value,
    #[cfg(test)]
    #[serde(skip)]
    flag: u8,
}

impl Error {
    #[must_use]
    pub fn msg<M>(msg: M) -> Self
    where
        M: fmt::Display,
    {
        Self {
            error: msg.to_string().into(),
            #[cfg(test)]
            flag: u8::MAX,
        }
    }
}

impl<T> From<T> for Error
where
    T: core::error::Error,
{
    fn from(error: T) -> Self {
        // we can maybe introduce specialized behaviour for `T: Error`
        // where (IF) the error is being serialized as a return type,
        // we can force a log of it's Debug representation, while we
        // serialize it's Display representation
        // ? but what if the dev already called `env::log` explicitly?
        Self::msg(error)
    }
}

#[doc(hidden)]
pub mod __private {
    use std::fmt::{self, Arguments};

    use serde::Serialize;

    use super::Error;

    #[derive(Debug)]
    pub struct Wrap<T>(pub T);

    pub trait ViaStr {
        fn into_error(&self) -> Error;
    }

    impl ViaStr for &&&Wrap<&str> {
        fn into_error(&self) -> Error {
            let Wrap(value) = self;
            Error {
                error: (*value).into(),
                #[cfg(test)]
                flag: 0,
            }
        }
    }

    pub trait ViaArguments {
        fn into_error(&self) -> Error;
    }

    impl ViaArguments for &&Wrap<Arguments<'_>> {
        fn into_error(&self) -> Error {
            let Wrap(value) = self;

            let Some(msg) = value.as_str() else {
                return Error {
                    error: fmt::format(*value).into(),
                    #[cfg(test)]
                    flag: 1,
                };
            };

            (&&&&Wrap(msg)).into_error()
        }
    }

    pub trait ViaSerialize {
        fn into_error(&self) -> Error;
    }

    impl<T> ViaSerialize for &Wrap<T>
    where
        T: Serialize,
    {
        fn into_error(&self) -> Error {
            let Wrap(value) = self;
            Error {
                error: serde_json::json!(value),
                #[cfg(test)]
                flag: 2,
            }
        }
    }

    pub trait ViaError {
        fn into_error(&self) -> Error;
    }

    impl<T> ViaError for Wrap<T>
    where
        T: core::error::Error,
    {
        fn into_error(&self) -> Error {
            let Wrap(value) = self;
            Error {
                error: value.to_string().into(),
                #[cfg(test)]
                flag: 3,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::__private::*;
    use super::*;
    use crate::app;

    fn _case1() -> app::Result<()> {
        app::bail!("something happened");
    }

    fn _case2() -> app::Result<()> {
        app::bail!(Errorlike);
    }

    fn _case3() -> app::Result<()> {
        app::bail!(SerializableError {
            name: "something happened".to_string()
        });
    }

    fn _case4() -> app::Result<()> {
        let arg = "happened";

        app::bail!(format_args!("something {arg}"));
    }

    // fn _case5() -> app::Result<()> {
    //     app::bail!(Displayable("something happened"));
    // }

    macro_rules! into_error {
        ($err:expr) => {
            (&&&&Wrap($err)).into_error()
        };
    }

    #[test]
    fn test_specialization() {
        let err = "beltalowda";

        let error = into_error!(err);

        dbg!(&error);
        assert_eq!(error.flag, 0); // used `ViaStr`

        // ---

        let err = SerializableError {
            name: "felota".to_string(),
        };

        let error = into_error!(err);

        dbg!(&error);
        assert_eq!(error.flag, 2); // used `ViaSerialize`

        // ---

        let err = Errorlike;

        let error = into_error!(err);

        dbg!(&error);
        assert_eq!(error.flag, 3); // used `ViaError`

        // ---

        let err = format_args!("beratna");

        let error = into_error!(err);

        dbg!(&error);
        assert_eq!(error.flag, 0); // used `ViaStr` via `ViaArguments`

        // ---

        let field = Displayable("bosmang");

        let error = into_error!(format_args!("{field}"));

        dbg!(&error);
        assert_eq!(error.flag, 1); // used `ViaArguments`

        // ---

        let field = Displayable("ke");

        let error = into_error!(format_args!("sasa {}", field));

        dbg!(&error);
        assert_eq!(error.flag, 1); // used `ViaArguments`
    }

    #[derive(Debug, Serialize)]
    struct SerializableError {
        name: String,
    }

    impl core::error::Error for SerializableError {}
    impl fmt::Display for SerializableError {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(f, "SerializableError")
        }
    }

    #[derive(Debug)]
    struct Errorlike;

    impl core::error::Error for Errorlike {}

    impl fmt::Display for Errorlike {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(f, "Errorlike")
        }
    }

    struct Displayable(&'static str);

    impl fmt::Display for Displayable {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.pad(self.0)
        }
    }
}
