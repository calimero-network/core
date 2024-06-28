use std::ops::Add;

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

impl<T: KeyComponent, U: KeyComponent> KeyComponents for (T, U)
where
    T::LEN: Add<U::LEN>,
    Sum<T::LEN, U::LEN>: ArrayLength,
{
    type LEN = Sum<T::LEN, U::LEN>;
}
