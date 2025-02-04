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

impl<T: KeyComponent, U: KeyComponent> KeyComponents for (T, U)
where
    T::LEN: Add<U::LEN, Output: ArrayLength>,
{
    type LEN = Sum<T::LEN, U::LEN>;
}

impl<T: KeyComponent, U: KeyComponent, V: KeyComponent> KeyComponents for (T, U, V)
where
    T::LEN: Add<U::LEN, Output: ArrayLength>,
    Sum<T::LEN, U::LEN>: Add<V::LEN, Output: ArrayLength>,
{
    type LEN = Sum<Sum<T::LEN, U::LEN>, V::LEN>;
}
