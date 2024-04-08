use calimero_sdk::app;

#[app::state]
struct MyType<'a>(&'a ());

#[app::logic]
impl<'a> MyType<'a> {
    pub fn method_00(self) {}
    pub fn method_01(self: Self) {}
    pub fn method_02(self: (Self)) {}
    pub fn method_03(self: (Self,)) {}
    pub fn method_04(mut self: Self) {}
    pub fn method_05(mut self: (Self)) {}
    pub fn method_06(self: &'a (Self,)) {}
    pub fn method_07(self: MyType<'a>) {}
    pub fn method_08(self: (MyType<'a>)) {}
    pub fn method_09(self: (MyType<'a>,)) {}
    pub fn method_10(mut self: MyType<'a>) {}
    pub fn method_11(mut self: (MyType<'a>)) {}
    pub fn method_12(mut self: (MyType<'a>,)) {}
    pub fn method_13(self: OtherType) {}
    pub fn method_14(self: (OtherType)) {}
    pub fn method_15(self: (OtherType,)) {}
    pub fn method_16(self: &OtherType) {}
    pub fn method_17(self: &(OtherType)) {}
    pub fn method_18(self: &(OtherType,)) {}
}

struct OtherType;

fn main() {}
