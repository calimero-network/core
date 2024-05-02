use calimero_sdk::app;

#[app::state]
struct MyType<'t, T, 'calimero>(&'t &'calimero T); // todo! what happens here?

#[app::logic]
impl<'t, T, 'calimero> MyType<'t, T, 'calimero> {
    // ignored because it's private
    fn method0<'k, K, 'v, V>(&self, tag: &'t T, key: &'k K, value: &'v V) {}
    pub fn method1<'k, K, 'v, V>(&self, tag: &'t T, key: &'k K, value: &'v V) {}
    pub fn method2<const N: usize>(&self, arr: [u8; N]) {}
}

#[app::logic]
impl<'t, T, 'x> MyType<'t, T, 'x> {
    pub fn method<'k, 'calimero>(&self, key: &'k str, value: &'calimero str) {}
}

fn main() {}
