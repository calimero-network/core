use calimero_sdk::app;

#[app::state]
struct MyType<'a>(&'a ());

#[app::logic]
impl<'a> MyType<'a> {
    // #[app::destroy]
    // pub fn method_00(self) {}
    pub fn method_01(&self) {}
    pub fn method_02(&mut self) {}
    // #[app::destroy]
    // pub fn method_03(self: Self) {}
    // #[app::destroy]
    // pub fn method_04(self: (Self)) {}
    // #[app::destroy]
    // pub fn method_05(mut self: Self) {}
    // #[app::destroy]
    // pub fn method_06(mut self: (Self)) {}
    // #[app::destroy]
    // pub fn method_07(self: &'a Self) {}
    // #[app::destroy]
    // pub fn method_08(self: &'a (Self)) {}
    pub fn method_09(mut self: &'a Self) {}
    pub fn method_10(mut self: &'a (Self)) {}
    pub fn method_11(self: &'a mut Self) {}
    pub fn method_12(self: &'a mut (Self)) {}
    pub fn method_13(mut self: &'a mut Self) {}
    pub fn method_14(mut self: &'a mut (Self)) {}
    // #[app::destroy]
    // pub fn method_15(self: MyType<'a>) {}
    // #[app::destroy]
    // pub fn method_16(self: (MyType<'a>)) {}
    // #[app::destroy]
    // pub fn method_17(mut self: MyType<'a>) {}
    // #[app::destroy]
    // pub fn method_18(mut self: (MyType<'a>)) {}
    pub fn method_19(self: &'a MyType<'a>) {}
    pub fn method_20(self: &'a (MyType<'a>)) {}
    pub fn method_21(mut self: &'a MyType<'a>) {}
    pub fn method_22(mut self: &'a (MyType<'a>)) {}
    pub fn method_23(self: &'a mut MyType<'a>) {}
    pub fn method_24(self: &'a mut (MyType<'a>)) {}
    pub fn method_25(mut self: &'a mut MyType<'a>) {}
    pub fn method_26(mut self: &'a mut (MyType<'a>)) {}
    // pub fn method_27(self: OtherType) {}
    // pub fn method_28(self: &'a OtherType) {}
}

struct OtherType;

fn main() {}
