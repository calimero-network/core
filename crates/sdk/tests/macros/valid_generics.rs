use calimero_sdk::app;

#[app::state]
struct MyType<'t>;

#[app::logic]
impl<'t> MyType<'t> {
    // ignored because it's private
    fn method0<'k, K, 'v, V, 'calimero>(
        &self,
        tag: &'t T,
        key: &'k K,
        value: &'v V,
        calimero: &'calimero str,
    ) {
    }
    pub fn method<'k, 'v>(&self, tag: &'t str, key: &'k str, value: &'v str) {}
}

fn main() {}
