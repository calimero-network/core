//! Tests for nested types in SDK macros
//!
//! This file tests that the SDK macros correctly handle nested structures,
//! including nested Option types, Vec of structs, and deeply nested types.

use calimero_sdk::app;

// Test nested Option types
#[app::state]
struct StateWithNestedOptions {
    nested_option: Option<Option<String>>,
    option_vec: Option<Vec<String>>,
    vec_option: Vec<Option<String>>,
}

#[app::logic]
impl StateWithNestedOptions {
    pub fn get_nested_option(&self) -> &Option<Option<String>> {
        &self.nested_option
    }

    pub fn set_nested_option(&mut self, value: Option<Option<String>>) {
        self.nested_option = value;
    }

    pub fn get_option_vec(&self) -> &Option<Vec<String>> {
        &self.option_vec
    }

    pub fn get_vec_option(&self) -> &Vec<Option<String>> {
        &self.vec_option
    }
}

// Test nested struct types
// Note: In production use, these nested types would need to implement
// serialization traits (e.g., BorshSerialize, BorshDeserialize) for
// actual runtime persistence. This test focuses on macro expansion.
struct InnerData {
    value: String,
    count: u64,
}

struct MiddleLayer {
    data: InnerData,
    items: Vec<InnerData>,
}

#[app::state]
struct StateWithNestedStructs {
    middle: MiddleLayer,
    optional_middle: Option<MiddleLayer>,
}

#[app::logic]
impl StateWithNestedStructs {
    pub fn get_middle(&self) -> &MiddleLayer {
        &self.middle
    }

    pub fn get_optional_middle(&self) -> &Option<MiddleLayer> {
        &self.optional_middle
    }
}

// Test nested tuple types
#[app::state]
struct StateWithTuples {
    simple_tuple: (String, u64),
    nested_tuple: ((String, u64), (bool, i32)),
    tuple_vec: Vec<(String, u64)>,
    option_tuple: Option<(String, Vec<u8>)>,
}

#[app::logic]
impl StateWithTuples {
    pub fn get_simple_tuple(&self) -> &(String, u64) {
        &self.simple_tuple
    }

    pub fn get_nested_tuple(&self) -> &((String, u64), (bool, i32)) {
        &self.nested_tuple
    }

    pub fn process_tuple_arg(&self, arg: (String, u64)) -> String {
        arg.0
    }

    pub fn process_nested_tuple_arg(&self, arg: ((String, u64), bool)) -> u64 {
        (arg.0).1
    }
}

// Test deeply nested types (3+ levels)
#[app::state]
struct DeeplyNested {
    deep: Option<Vec<Option<Vec<String>>>>,
    deep_tuple: Option<(Vec<Option<String>>, Option<Vec<u64>>)>,
}

#[app::logic]
impl DeeplyNested {
    pub fn get_deep(&self) -> &Option<Vec<Option<Vec<String>>>> {
        &self.deep
    }

    pub fn set_deep(&mut self, value: Option<Vec<Option<Vec<String>>>>) {
        self.deep = value;
    }
}

// Test nested types in method arguments and return types
#[app::state]
struct NestedMethodTypes;

#[app::logic]
impl NestedMethodTypes {
    pub fn nested_arg(&self, arg: Option<Vec<Option<String>>>) -> bool {
        arg.is_some()
    }

    pub fn nested_return(&self) -> Option<Vec<Option<String>>> {
        Some(vec![Some("test".to_string()), None])
    }

    pub fn nested_both(
        &self,
        arg: Vec<Option<(String, u64)>>,
    ) -> Option<Vec<Option<(String, u64)>>> {
        Some(arg)
    }
}

// Test array types with nested content
#[app::state]
struct StateWithArrays {
    fixed_array: [String; 3],
    nested_array: [Option<String>; 2],
}

#[app::logic]
impl StateWithArrays {
    pub fn get_fixed_array(&self) -> &[String; 3] {
        &self.fixed_array
    }

    pub fn get_nested_array(&self) -> &[Option<String>; 2] {
        &self.nested_array
    }
}

fn main() {}
