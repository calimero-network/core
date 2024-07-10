# Accountability rules

- Do not hoard tokens from any other parser, including `rustc`

  - Meaning, even in the case of errors, return all the tokens you received, and
    only highlight the ones that are problematic

- Collect all possible errors and report them in a single pass

- Account for as many patterns in argument positions as possible

- Sanitize `Self` early, to build our IR

- Support references, and encourage them to avoid copying

- No magic. Everything should be explicit.

  - Any behavior that affects the generated code should be part of the API. For
    example: `#[app::destroy]` as a macro doesn't do anything by itself, but
    informs the code generator to permit state destruction

- Support `#[app::*(crate = "foo")]` to allow referencing a custom `sdk` crate

- Sanitize attr arguments, account for all possible values

## Thoughts

- Do we really need to allocate in the codegen?

- Consider using traits to define app behavior to keep code generation simple

- Should we support consumption of `self` as a pattern for eventual state
  destruction?

  - It's more idiomatic, but could be a footgun

  - So maybe add an error, telling the user to either use `&self` or add the
    `#[app::destroy]` attribute

- How do we work out migration?

  - Can be tied into how we handle state reconciliation

## Test Cases

```rust
pub fn method(self, value: u32) {}
pub fn method(self, value: &str) -> Result<String> {}
pub fn method<'a>(self, value: &'a str) {}
pub fn method<'a>(self, _: &'a str) {}
pub fn method(self, (a, b): (u8, u8)) -> Result<(), &'static str> {}
pub fn method(self, MyType(a, b): MyType) -> Result<(), &'static str> {}
// try structs too
pub fn method(self, all @ MyType(opt @ Ok(a) | opt @ Err(a), b): MyType) -> Result<(), &'static str> {}
pub fn method(self, my_macro!(): MyType) -> Result<(), &'static str> {}
pub fn method(&self) {}
pub fn method(&mut self) {}
pub fn method(self: Self) {}
pub fn method(self: (Self)) {}
pub fn method(self: &Self) {}
pub fn method(self: &mut Self) {}
pub fn method(self: &mut (Self)) {}
pub fn method(self: &mut ActualSelfType) {}
pub fn method(self: &mut (ActualSelfType)) {}
pub fn method(self: Box<Self>) {} // should error
pub fn method(self: OtherType) {} // should error
impl X<T> {} // should error
impl X<'t> {
  pub fn method<'k, K, 'v, V>(&self, tag: &'t str, key: &'k K, value: &'v V) {} // should error on K, V
}
impl X<'t> {
  pub fn method(self: OtherType) {} // should error, suggest replace with `Self` or `X<'t>`
}
impl (MyType, MyType) {} // should error
impl [MyType] {} // should error
impl &MyType {} // should error
```
