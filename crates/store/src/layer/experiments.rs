#![allow(unused, reason = "Will be used in the future")]

// Experiments at specialized mutable behavior over any `WriteLayer`
// Baking expectations of interior mutability into the type system

mod layer {
    use crate::layer::experiments::layer::private::Sealed;

    pub struct Interior;

    pub struct Identity;

    mod private {
        pub trait Sealed {}
    }

    pub trait Discriminant: Sealed {
        type Ref<'a, T>
        where
            T: ?Sized + 'a;
    }

    impl Sealed for Interior {}
    impl Discriminant for Interior {
        type Ref<'a, T>
            = &'a T
        where
            T: ?Sized + 'a;
    }

    impl Sealed for Identity {}
    impl Discriminant for Identity {
        type Ref<'a, T>
            = &'a mut T
        where
            T: ?Sized + 'a;
    }

    pub trait WriteLayer<D: Discriminant> {
        fn put(this: D::Ref<'_, Self>);
    }

    // forbid overloaded mutable behavior
    impl<T: WriteLayer<Interior>> WriteLayer<Identity> for T {
        fn put(this: &mut Self) {
            T::put(this);
        }
    }

    pub trait WriteLayerMut: WriteLayer<Identity> {
        fn put(&mut self);
    }

    pub trait WriteLayerRef: WriteLayer<Interior> {
        fn put(&self);
    }

    impl<T: WriteLayer<Interior>> WriteLayerRef for T {
        fn put(&self) {
            T::put(self);
        }
    }

    impl<T: WriteLayer<Identity>> WriteLayerMut for T {
        fn put(&mut self) {
            T::put(self);
        }
    }
}

use layer::{Identity, Interior, WriteLayer, WriteLayerMut, WriteLayerRef};

struct SomeLayer;

impl WriteLayer<Identity> for SomeLayer {
    fn put(this: &mut Self) {
        //        ^^^~ requires `mut`, yay
    }
}

struct OtherLayer;

impl WriteLayer<Interior> for OtherLayer {
    fn put(this: &Self) {
        //        ^~~~ interior mutability
    }
}

fn test() {
    let mut some_layer = SomeLayer;
    //  ^^^~ requires `mut`, yay

    some_layer.put();

    let other_layer = OtherLayer;

    other_layer.put();
    //         ^~~~ interior mutability
}
