#![allow(dead_code, reason = "The corpus types are described, never built")]
//! Golden shapes: what `#[derive(AbiType)]` puts in the manifest for each kind
//! of type an app can declare.
//!
//! Every case pins the JSON the derive produces, so a change in the described
//! shape shows up as a diff here rather than in an app's `abi.json`. The corpus
//! types are private on purpose: the derive has to work on non-`pub` types
//! (apps put private structs in ABI positions).

use calimero_sdk::abi::{AbiEvents, AbiType, TypeRegistry};
use calimero_sdk::app;
use calimero_sdk::serde_json::{json, to_value, Value};

/// The type definitions `T` registers, as JSON.
fn derived<T: AbiType>() -> Value {
    let mut reg = TypeRegistry::new();
    <T as AbiType>::register(&mut reg);
    to_value(reg.into_types()).expect("type definitions serialize")
}

#[derive(AbiType)]
struct Item {
    a: Option<String>,
    b: u32,
}

// `nullable` marks an `Option` field; the type itself is the unwrapped one, and
// a non-optional field carries no `nullable` key at all.
#[test]
fn named_struct_with_option_field() {
    assert_eq!(
        derived::<Item>(),
        json!({
            "Item": {
                "kind": "record",
                "fields": [
                    { "name": "a", "nullable": true, "type": { "kind": "string" } },
                    { "name": "b", "type": { "kind": "u32" } },
                ],
            }
        })
    );
}

#[derive(AbiType)]
struct Sizes {
    n: usize,
    i: isize,
}

// The manifest describes the wasm32 build, so a pointer-sized integer is 32-bit
// however wide it is where the description is assembled.
#[test]
fn pointer_sized_integers_describe_as_wasm32() {
    assert_eq!(
        derived::<Sizes>(),
        json!({
            "Sizes": {
                "kind": "record",
                "fields": [
                    { "name": "n", "type": { "kind": "u32" } },
                    { "name": "i", "type": { "kind": "i32" } },
                ],
            }
        })
    );
}

#[derive(AbiType)]
struct Nt(String);

#[test]
fn one_field_tuple_struct_is_an_alias() {
    assert_eq!(
        derived::<Nt>(),
        json!({ "Nt": { "kind": "alias", "target": { "kind": "string" } } })
    );
}

#[derive(AbiType)]
struct Pair(Option<String>, u32);

#[test]
fn multi_field_tuple_struct_repeats_the_unnamed_field_name() {
    assert_eq!(
        derived::<Pair>(),
        json!({
            "Pair": {
                "kind": "record",
                "fields": [
                    { "name": "unnamed", "nullable": true, "type": { "kind": "string" } },
                    { "name": "unnamed", "type": { "kind": "u32" } },
                ],
            }
        })
    );
}

#[derive(AbiType)]
struct Marker;

#[test]
fn unit_struct_is_an_empty_record() {
    assert_eq!(
        derived::<Marker>(),
        json!({ "Marker": { "kind": "record", "fields": [] } })
    );
}

#[derive(AbiType)]
enum Act {
    Ping,
    Named(String),
    Single { x: u32 },
    Multi { x: u32, y: Option<String> },
    Pair(String, u32),
}

// A unit variant has no payload and a single unnamed field inlines its type;
// everything else gets a synthesized `{Enum}_{Variant}` record, whose fields
// carry no `nullable` key even for an `Option`.
#[test]
fn enum_variants_and_their_synthesized_payload_records() {
    assert_eq!(
        derived::<Act>(),
        json!({
            "Act": {
                "kind": "variant",
                "variants": [
                    { "name": "Ping" },
                    { "name": "Named", "payload": { "kind": "string" } },
                    { "name": "Single", "payload": { "$ref": "Act_Single" } },
                    { "name": "Multi", "payload": { "$ref": "Act_Multi" } },
                    { "name": "Pair", "payload": { "$ref": "Act_Pair" } },
                ],
            },
            "Act_Single": {
                "kind": "record",
                "fields": [{ "name": "x", "type": { "kind": "u32" } }],
            },
            "Act_Multi": {
                "kind": "record",
                "fields": [
                    { "name": "x", "type": { "kind": "u32" } },
                    { "name": "y", "type": { "kind": "string" } },
                ],
            },
            "Act_Pair": {
                "kind": "record",
                "fields": [
                    { "name": "field_0", "type": { "kind": "string" } },
                    { "name": "field_1", "type": { "kind": "u32" } },
                ],
            },
        })
    );
}

#[derive(AbiType)]
struct Inner {
    v: u64,
}

#[derive(AbiType)]
struct Outer {
    inner: Inner,
    tag: String,
}

#[test]
fn nested_struct_registers_both_definitions() {
    assert_eq!(
        derived::<Outer>(),
        json!({
            "Inner": {
                "kind": "record",
                "fields": [{ "name": "v", "type": { "kind": "u64" } }],
            },
            "Outer": {
                "kind": "record",
                "fields": [
                    { "name": "inner", "type": { "$ref": "Inner" } },
                    { "name": "tag", "type": { "kind": "string" } },
                ],
            },
        })
    );
}

#[derive(AbiType)]
struct Wrapper<T> {
    value: T,
}

// A generic type is described at its instantiation: one definition named after
// the bare identifier, with the parameter already resolved.
#[test]
fn generic_struct_registers_its_instantiated_shape() {
    assert_eq!(
        derived::<Wrapper<String>>(),
        json!({
            "Wrapper": {
                "kind": "record",
                "fields": [{ "name": "value", "type": { "kind": "string" } }],
            }
        })
    );
}

#[derive(AbiType, calimero_sdk::serde::Serialize)]
#[serde(crate = "calimero_sdk::serde")]
pub struct Payload {
    v: u32,
}

#[app::event]
pub enum Event {
    Ping,
    Named(String),
    Single { x: u32 },
    Multi { x: u32, y: Option<String> },
    Pair(String, u32),
    Nested(Payload),
}

// `#[app::event]` describes the enum through `AbiEvents`, not as a named type:
// only the payload records reach `types`, in declaration order.
#[test]
fn event_enum_entries_and_payload_records() {
    let mut reg = TypeRegistry::new();
    let events = <Event as AbiEvents>::abi_events(&mut reg);

    assert_eq!(
        to_value(events).expect("events serialize"),
        json!([
            { "name": "Ping" },
            { "name": "Named", "payload": { "kind": "string" } },
            { "name": "Single", "payload": { "$ref": "Event_Single" } },
            { "name": "Multi", "payload": { "$ref": "Event_Multi" } },
            { "name": "Pair", "payload": { "$ref": "Event_Pair" } },
            { "name": "Nested", "payload": { "$ref": "Payload" } },
        ])
    );
    assert_eq!(
        to_value(reg.into_types()).expect("type definitions serialize"),
        json!({
            "Event_Single": {
                "kind": "record",
                "fields": [{ "name": "x", "type": { "kind": "u32" } }],
            },
            "Event_Multi": {
                "kind": "record",
                "fields": [
                    { "name": "x", "type": { "kind": "u32" } },
                    { "name": "y", "type": { "kind": "string" } },
                ],
            },
            "Event_Pair": {
                "kind": "record",
                "fields": [
                    { "name": "field_0", "type": { "kind": "string" } },
                    { "name": "field_1", "type": { "kind": "u32" } },
                ],
            },
            "Payload": {
                "kind": "record",
                "fields": [{ "name": "v", "type": { "kind": "u32" } }],
            },
        })
    );
}

#[derive(AbiType)]
struct Borrowed<'a> {
    v: &'a Option<String>,
}

// A reference is transparent to the description, so a borrowed Option is as
// nullable as an owned one.
#[test]
fn option_behind_a_reference_is_nullable() {
    assert_eq!(
        derived::<Borrowed<'static>>(),
        json!({
            "Borrowed": {
                "kind": "record",
                "fields": [
                    { "name": "v", "nullable": true, "type": { "kind": "string" } },
                ],
            }
        })
    );
}

#[derive(AbiType)]
#[abi(name = "Renamed")]
struct Original {
    v: u32,
}

// `#[abi(name)]` sets the manifest identity, so two types sharing a Rust
// identifier (or two shapes wanting distinct public names) can both register.
#[test]
fn abi_name_attribute_renames_the_definition() {
    let mut reg = TypeRegistry::new();
    let r = <Original as AbiType>::type_ref(&mut reg);
    assert_eq!(
        to_value(&r).expect("ref serializes"),
        json!({ "$ref": "Renamed" })
    );
    assert_eq!(
        to_value(reg.into_types()).expect("types serialize"),
        json!({
            "Renamed": {
                "kind": "record",
                "fields": [ { "name": "v", "type": { "kind": "u32" } } ],
            }
        })
    );
}
