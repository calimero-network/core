use core::ops::Add;

use generic_array::typenum::Sum;
use generic_array::ArrayLength;

pub trait KeyComponent {
    type LEN: ArrayLength;
}

pub trait KeyComponents {
    type LEN: ArrayLength;
}

impl<T: KeyComponent> KeyComponents for T {
    type LEN = T::LEN;
}

impl<T: KeyComponent> KeyComponents for (T,) {
    type LEN = T::LEN;
}

impl<T: KeyComponents, U: KeyComponents> KeyComponents for (T, U)
where
    T::LEN: Add<U::LEN, Output: ArrayLength>,
{
    type LEN = Sum<T::LEN, U::LEN>;
}

macro_rules! impl_key_components {
    ($a:ident, $b:ident) => {};
    ($t:ident, $($ts:ident),+) => {
        impl<$t: KeyComponents, $($ts: KeyComponents),+> KeyComponents for ($t, $($ts),+)
        where
            impl_key_components!(@ $t, $($ts),+): KeyComponents,
        {
            type LEN = <impl_key_components!(@ $t, $($ts),+) as KeyComponents>::LEN;
        }

        impl_key_components!($($ts),+);
    };
    (@ $t:ident, $($ts:ident),+) => {
        ($t, impl_key_components!(@ $($ts),+))
    };
    (@ $t:ident) => {
        $t
    };
}

impl_key_components!(
    T01, T02, T03, T04, T05, T06, T07, T08, T09, T10, T11, T12, T13, T14, T15, T16
);
