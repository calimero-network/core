use std::collections::BTreeMap;

use calimero_wasm_abi::abi_type::{AbiType, TypeRegistry};
use calimero_wasm_abi::schema::{CollectionType, Field, ScalarType, TypeDef, TypeRef};

/// A type that contains a list of itself. `register` must terminate.
struct Node;

impl AbiType for Node {
    fn type_ref(reg: &mut TypeRegistry) -> TypeRef {
        Self::register(reg);
        TypeRef::Reference {
            ref_: "Node".to_owned(),
        }
    }

    fn register(reg: &mut TypeRegistry) {
        reg.define("Node", |reg| TypeDef::Record {
            fields: vec![Field {
                name: "children".to_owned(),
                // recurses into Node again
                type_: TypeRef::Collection {
                    collection: CollectionType::List {
                        items: Box::new(<Node as AbiType>::type_ref(reg)),
                    },
                    crdt_type: None,
                    inner_type: None,
                },
                nullable: None,
            }],
        });
    }
}

#[test]
fn recursive_type_terminates_and_is_defined_once() {
    let mut reg = TypeRegistry::new();
    let r = <Node as AbiType>::type_ref(&mut reg);
    assert_eq!(
        r,
        TypeRef::Reference {
            ref_: "Node".to_owned()
        }
    );

    let types: BTreeMap<String, TypeDef> = reg.into_types();
    assert_eq!(types.len(), 1, "Node must be defined exactly once");
    assert!(types.contains_key("Node"));
}

#[test]
fn define_is_idempotent_by_name() {
    let mut reg = TypeRegistry::new();
    reg.define("A", |_| TypeDef::Record { fields: vec![] });
    reg.define("A", |_| TypeDef::Record { fields: vec![] });
    assert_eq!(reg.into_types().len(), 1);
}

#[test]
#[should_panic(expected = "ABI type name collision: A")]
fn same_name_with_a_different_shape_is_a_collision() {
    let mut reg = TypeRegistry::new();
    reg.define("A", |_| TypeDef::Record { fields: vec![] });
    reg.define("A", |_| TypeDef::Bytes {
        size: Some(32),
        encoding: None,
    });
}

#[test]
fn scalar_types_register_no_definition() {
    let mut reg = TypeRegistry::new();
    let r = <String as AbiType>::type_ref(&mut reg);
    assert_eq!(r, TypeRef::Scalar(ScalarType::String));
    assert!(
        reg.into_types().is_empty(),
        "scalars must not create named types"
    );
}
