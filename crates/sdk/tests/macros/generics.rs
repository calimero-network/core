use calimero_sdk::app;

#[app::state]
struct MyType<'t, T>(&'t T);

trait MyTrait {}

#[app::logic]
impl<'t, T> MyType<'t, T> {
    // ignored because it's private
    fn method0<'k, K, 'v, V>(&self, tag: &'t T, key: &'k K, value: &'v V) {}
    pub fn method1<'k, K, 'v, V>(&self, tag: &'t T, key: &'k K, value: &'v V) {}
    pub fn method2<const N: usize>(&self, arr: [u8; N]) {}
}

fn main() {}
