macro_rules! transport {
    // Type declaration
    (
        $(#[$meta:meta])*
        pub type $name:ident = ( $($t:ty),* $(,)? );
    ) => {
        $(#[$meta])*
        pub type $name = transport!(@nest_types $($t),*);
    };

    (@nest_types $first:ty, $($rest:ty),*) => {
        Both<$first, transport!(@nest_types $($rest),*)>
    };
    (@nest_types $last:ty) => { $last };

    // Instance creation
    ( $($args:expr),* $(,)? ) => {{
        transport!(@nest_instances $($args),*)
    }};

    (@nest_instances $first:expr, $($rest:expr),*) => {{
        Both {
            left: $first,
            right: transport!(@nest_instances $($rest),*)
        }
    }};
    (@nest_instances $last:expr) => { $last };
}
pub(crate) use transport;
