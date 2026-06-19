//! `#[app::state]` / `#[app::logic]` correctly handle deeply-nested types.
//!
//! Each scenario keeps its original fields; bare value types are wrapped in
//! `LwwRegister<T>` — the CRDT form for a single value. `LwwRegister<T>` derefs
//! to `T`, so the `&self.field` accessors compile unchanged; mutation goes
//! through `.set(...)`.

use calimero_sdk::app;
use calimero_sdk::borsh::{BorshDeserialize, BorshSerialize};
use calimero_storage::collections::LwwRegister;

// Test nested Option types
#[app::state]
struct StateWithNestedOptions {
    nested_option: LwwRegister<Option<Option<String>>>,
    option_vec: LwwRegister<Option<Vec<String>>>,
    vec_option: LwwRegister<Vec<Option<String>>>,
}

#[app::logic]
impl StateWithNestedOptions {
    #[app::init]
    pub fn init() -> StateWithNestedOptions {
        StateWithNestedOptions {
            nested_option: LwwRegister::new(None),
            option_vec: LwwRegister::new(None),
            vec_option: LwwRegister::new(Vec::new()),
        }
    }

    pub fn get_nested_option(&self) -> &Option<Option<String>> {
        &self.nested_option
    }

    pub fn set_nested_option(&mut self, value: Option<Option<String>>) {
        self.nested_option.set(value);
    }

    pub fn get_option_vec(&self) -> &Option<Vec<String>> {
        &self.option_vec
    }

    pub fn get_vec_option(&self) -> &Vec<Option<String>> {
        &self.vec_option
    }
}

// Test nested struct types — user types nested inside CRDT fields must be
// borsh-(de)serializable and `Clone` (the `LwwRegister<T>: Mergeable` bound).
#[derive(Clone, BorshSerialize, BorshDeserialize)]
#[borsh(crate = "calimero_sdk::borsh")]
struct InnerData {
    value: String,
    count: u64,
}

#[derive(Clone, BorshSerialize, BorshDeserialize)]
#[borsh(crate = "calimero_sdk::borsh")]
struct MiddleLayer {
    data: InnerData,
    items: Vec<InnerData>,
}

#[app::state]
struct StateWithNestedStructs {
    middle: LwwRegister<MiddleLayer>,
    optional_middle: LwwRegister<Option<MiddleLayer>>,
}

#[app::logic]
impl StateWithNestedStructs {
    #[app::init]
    pub fn init() -> StateWithNestedStructs {
        StateWithNestedStructs {
            middle: LwwRegister::new(MiddleLayer {
                data: InnerData {
                    value: String::new(),
                    count: 0,
                },
                items: Vec::new(),
            }),
            optional_middle: LwwRegister::new(None),
        }
    }

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
    simple_tuple: LwwRegister<(String, u64)>,
    nested_tuple: LwwRegister<((String, u64), (bool, i32))>,
    tuple_vec: LwwRegister<Vec<(String, u64)>>,
    option_tuple: LwwRegister<Option<(String, Vec<u8>)>>,
}

#[app::logic]
impl StateWithTuples {
    #[app::init]
    pub fn init() -> StateWithTuples {
        StateWithTuples {
            simple_tuple: LwwRegister::new((String::new(), 0)),
            nested_tuple: LwwRegister::new(((String::new(), 0), (false, 0))),
            tuple_vec: LwwRegister::new(Vec::new()),
            option_tuple: LwwRegister::new(None),
        }
    }

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
    deep: LwwRegister<Option<Vec<Option<Vec<String>>>>>,
    deep_tuple: LwwRegister<Option<(Vec<Option<String>>, Option<Vec<u64>>)>>,
}

#[app::logic]
impl DeeplyNested {
    #[app::init]
    pub fn init() -> DeeplyNested {
        DeeplyNested {
            deep: LwwRegister::new(None),
            deep_tuple: LwwRegister::new(None),
        }
    }

    pub fn get_deep(&self) -> &Option<Vec<Option<Vec<String>>>> {
        &self.deep
    }

    pub fn set_deep(&mut self, value: Option<Vec<Option<Vec<String>>>>) {
        self.deep.set(value);
    }
}

// Test nested types in method arguments and return types
#[app::state]
struct NestedMethodTypes;

#[app::logic]
impl NestedMethodTypes {
    #[app::init]
    pub fn init() -> NestedMethodTypes {
        NestedMethodTypes
    }

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
    fixed_array: LwwRegister<[String; 3]>,
    nested_array: LwwRegister<[Option<String>; 2]>,
}

#[app::logic]
impl StateWithArrays {
    #[app::init]
    pub fn init() -> StateWithArrays {
        StateWithArrays {
            fixed_array: LwwRegister::new([String::new(), String::new(), String::new()]),
            nested_array: LwwRegister::new([None, None]),
        }
    }

    pub fn get_fixed_array(&self) -> &[String; 3] {
        &self.fixed_array
    }

    pub fn get_nested_array(&self) -> &[Option<String>; 2] {
        &self.nested_array
    }
}

fn main() {}
