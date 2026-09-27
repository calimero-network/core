#![allow(
    dead_code,
    reason = "The corpus types are described and serialized, never read"
)]
//! The ABI describes what serde writes. Each case serializes a real value and
//! checks it against the manifest with `fits`, a reference reading of the ABI's
//! JSON wire rules, so the description cannot drift from serde.

use std::collections::BTreeMap;

use calimero_sdk::abi::{
    AbiEvents, AbiType, CollectionType, Field, ScalarType, TypeDef, TypeRef, TypeRegistry, Variant,
};
use calimero_sdk::app;
use calimero_sdk::serde::{Serialize, Serializer};
use calimero_sdk::serde_json::{json, to_value, Value};

type Types = BTreeMap<String, TypeDef>;

fn described<T: AbiType>() -> (TypeRef, Types) {
    let mut reg = TypeRegistry::new();
    let ty = <T as AbiType>::type_ref(&mut reg);
    (ty, reg.into_types())
}

fn assert_wire<T: AbiType + Serialize>(sample: &T) {
    let (ty, types) = described::<T>();
    let value = to_value(sample).expect("sample serializes");
    assert!(
        fits(&value, &ty, &types),
        "serde wrote {value}, the ABI describes {}",
        to_value(&types).expect("types serialize")
    );
}

fn fits(value: &Value, ty: &TypeRef, types: &Types) -> bool {
    match ty {
        TypeRef::Reference { ref_ } => types
            .get(ref_)
            .is_some_and(|def| fits_def(value, def, types)),
        TypeRef::Scalar(scalar) => match scalar {
            ScalarType::Bool => value.is_boolean(),
            ScalarType::I32 | ScalarType::I64 => value.is_i64(),
            ScalarType::U32 | ScalarType::U64 => value.is_u64(),
            ScalarType::F32 | ScalarType::F64 => value.is_number(),
            ScalarType::String => value.is_string(),
            ScalarType::Bytes { .. } => value
                .as_array()
                .is_some_and(|all| all.iter().all(Value::is_u64)),
            ScalarType::Unit => value.is_null(),
        },
        TypeRef::Collection { collection, .. } => match collection {
            CollectionType::List { items } => value
                .as_array()
                .is_some_and(|all| all.iter().all(|v| fits(v, items, types))),
            CollectionType::Map { value: of, .. } => value
                .as_object()
                .is_some_and(|all| all.values().all(|v| fits(v, of, types))),
            CollectionType::Record { fields } => fits_record(value, fields, None, types),
            CollectionType::Tuple { elements } => value.as_array().is_some_and(|all| {
                all.len() == elements.len()
                    && all.iter().zip(elements).all(|(v, t)| fits(v, t, types))
            }),
        },
    }
}

/// Every key is a described field (or the enum tag), and every field is present
/// unless it is nullable.
fn fits_record(value: &Value, fields: &[Field], tag: Option<&str>, types: &Types) -> bool {
    let Some(object) = value.as_object() else {
        return false;
    };
    let known = object
        .keys()
        .all(|key| Some(key.as_str()) == tag || fields.iter().any(|field| &field.name == key));
    known
        && fields.iter().all(|field| match object.get(&field.name) {
            None | Some(Value::Null) => field.nullable == Some(true),
            Some(v) => fits(v, &field.type_, types),
        })
}

fn fits_def(value: &Value, def: &TypeDef, types: &Types) -> bool {
    match def {
        TypeDef::Record { fields, .. } => fits_record(value, fields, None, types),
        TypeDef::Alias { target, .. } => fits(value, target, types),
        TypeDef::Bytes { .. } => value.as_array().is_some(),
        TypeDef::Variant {
            variants,
            tag,
            content,
            untagged,
            ..
        } => variants.iter().any(|variant| {
            fits_variant(
                value,
                variant,
                tag.as_deref(),
                content.as_deref(),
                *untagged,
                types,
            )
        }),
    }
}

fn fits_variant(
    value: &Value,
    variant: &Variant,
    tag: Option<&str>,
    content: Option<&str>,
    untagged: bool,
    types: &Types,
) -> bool {
    let payload = variant.payload.as_ref();
    if untagged {
        return payload.map_or(value.is_null(), |p| fits(value, p, types));
    }
    let Some(tag) = tag else {
        return match payload {
            None => value.as_str() == Some(variant.name.as_str()),
            Some(p) => value.as_object().is_some_and(|object| {
                object.len() == 1
                    && object
                        .get(&variant.name)
                        .is_some_and(|inner| fits(inner, p, types))
            }),
        };
    };
    let Some(object) = value.as_object() else {
        return false;
    };
    if object.get(tag).and_then(Value::as_str) != Some(variant.name.as_str()) {
        return false;
    }
    match (payload, content) {
        (None, _) => object.len() == 1,
        (Some(p), Some(content)) => {
            object.len() == 2
                && object
                    .get(content)
                    .is_some_and(|inner| fits(inner, p, types))
        }
        (Some(TypeRef::Reference { ref_ }), None) => matches!(
            types.get(ref_),
            Some(TypeDef::Record { fields, .. }) if fits_record(value, fields, Some(tag), types)
        ),
        (Some(_), None) => false,
    }
}

// mero-design's `Member` shape: camelCase keys, an explicit rename, a skipped
// cache, and a raw identifier.
#[derive(AbiType, Serialize)]
#[serde(crate = "calimero_sdk::serde", rename_all = "camelCase")]
struct Member {
    id: String,
    joined_at: u64,
    avatar: Option<String>,
    #[serde(rename = "blobId", default, skip_serializing_if = "String::is_empty")]
    blob_id: String,
    #[serde(skip)]
    cache: u32,
    r#type: String,
}

#[test]
fn record_fields_carry_their_wire_names() {
    let (_, types) = described::<Member>();
    assert_eq!(
        to_value(&types).unwrap(),
        json!({
            "Member": {
                "kind": "record",
                "fields": [
                    { "name": "id", "type": { "kind": "string" } },
                    { "name": "joinedAt", "type": { "kind": "u64" } },
                    { "name": "avatar", "nullable": true, "type": { "kind": "string" } },
                    { "name": "blobId", "type": { "kind": "string" } },
                    { "name": "type", "type": { "kind": "string" } },
                ],
            }
        })
    );
    assert_wire(&Member {
        id: "m1".to_owned(),
        joined_at: 7,
        avatar: None,
        blob_id: "b1".to_owned(),
        cache: 3,
        r#type: "admin".to_owned(),
    });
}

#[test]
fn the_oracle_rejects_rust_spelled_keys() {
    let (ty, types) = described::<Member>();
    let rust_spelled = json!({ "id": "m1", "joined_at": 7, "blobId": "b1", "type": "admin" });
    assert!(!fits(&rust_spelled, &ty, &types));
}

fn as_hex<S: Serializer>(bytes: &[u8; 2], serializer: S) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(&format!("{:02x}{:02x}", bytes[0], bytes[1]))
}

// scaffolding-e2e's `FileRecord.blob_id`: a custom serializer writes a string.
#[derive(AbiType, Serialize)]
#[serde(crate = "calimero_sdk::serde")]
struct Blob {
    #[serde(serialize_with = "as_hex")]
    #[abi(as = String)]
    id: [u8; 2],
}

#[test]
fn abi_as_names_the_type_a_custom_serializer_writes() {
    let (_, types) = described::<Blob>();
    assert_eq!(
        to_value(&types).unwrap(),
        json!({ "Blob": { "kind": "record", "fields": [{ "name": "id", "type": { "kind": "string" } }] } })
    );
    assert_wire(&Blob { id: [0xab, 0x01] });
}

macro_rules! field_rule {
    ($name:ident, $rule:tt) => {
        #[derive(AbiType, Serialize)]
        #[serde(crate = "calimero_sdk::serde", rename_all = $rule)]
        struct $name {
            some_field_name: u32,
        }
    };
}

field_rule!(FieldLower, "lowercase");
field_rule!(FieldUpper, "UPPERCASE");
field_rule!(FieldPascal, "PascalCase");
field_rule!(FieldCamel, "camelCase");
field_rule!(FieldSnake, "snake_case");
field_rule!(FieldScreamingSnake, "SCREAMING_SNAKE_CASE");
field_rule!(FieldKebab, "kebab-case");
field_rule!(FieldScreamingKebab, "SCREAMING-KEBAB-CASE");

#[test]
fn every_field_rename_rule_matches_serde() {
    assert_wire(&FieldLower { some_field_name: 1 });
    assert_wire(&FieldUpper { some_field_name: 1 });
    assert_wire(&FieldPascal { some_field_name: 1 });
    assert_wire(&FieldCamel { some_field_name: 1 });
    assert_wire(&FieldSnake { some_field_name: 1 });
    assert_wire(&FieldScreamingSnake { some_field_name: 1 });
    assert_wire(&FieldKebab { some_field_name: 1 });
    assert_wire(&FieldScreamingKebab { some_field_name: 1 });
}

// mero-design's `ElementData`: internally tagged on `kind`, variants lowercased,
// some fields renamed and some skipped when empty.
#[derive(AbiType, Serialize)]
#[serde(crate = "calimero_sdk::serde", rename_all = "lowercase", tag = "kind")]
enum ElementData {
    Rect,
    Line {
        #[serde(default, skip_serializing_if = "String::is_empty")]
        points: String,
    },
    Text {
        content: String,
        #[serde(rename = "fontSize")]
        font_size: u32,
        bold: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        text_align: Option<String>,
    },
    Image {
        #[serde(rename = "naturalWidth")]
        natural_width: u32,
        #[serde(rename = "blobId", default, skip_serializing_if = "String::is_empty")]
        blob_id: String,
    },
}

// mero-design's `Element`, the `add_element` argument.
#[derive(AbiType, Serialize)]
#[serde(crate = "calimero_sdk::serde", rename_all = "camelCase")]
struct Element {
    id: String,
    data: ElementData,
    stroke_width: u32,
    shadow_color: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    corner_radius: Option<u32>,
}

#[test]
fn an_internally_tagged_enum_is_described_as_serde_writes_it() {
    let (_, types) = described::<ElementData>();
    assert_eq!(
        to_value(&types).unwrap(),
        json!({
            "ElementData": {
                "kind": "variant",
                "tag": "kind",
                "variants": [
                    { "name": "rect" },
                    { "name": "line", "payload": { "$ref": "ElementData_Line" } },
                    { "name": "text", "payload": { "$ref": "ElementData_Text" } },
                    { "name": "image", "payload": { "$ref": "ElementData_Image" } },
                ],
            },
            "ElementData_Line": {
                "kind": "record",
                "fields": [{ "name": "points", "type": { "kind": "string" } }],
            },
            "ElementData_Text": {
                "kind": "record",
                "fields": [
                    { "name": "content", "type": { "kind": "string" } },
                    { "name": "fontSize", "type": { "kind": "u32" } },
                    { "name": "bold", "type": { "kind": "bool" } },
                    { "name": "text_align", "nullable": true, "type": { "kind": "string" } },
                ],
            },
            "ElementData_Image": {
                "kind": "record",
                "fields": [
                    { "name": "naturalWidth", "type": { "kind": "u32" } },
                    { "name": "blobId", "type": { "kind": "string" } },
                ],
            },
        })
    );
}

#[test]
fn add_element_arguments_fit_the_abi() {
    for data in [
        ElementData::Rect,
        ElementData::Line {
            points: "0,0 10,10".to_owned(),
        },
        ElementData::Text {
            content: "hi".to_owned(),
            font_size: 12,
            bold: true,
            text_align: None,
        },
        ElementData::Image {
            natural_width: 64,
            blob_id: "b1".to_owned(),
        },
    ] {
        assert_wire(&Element {
            id: "e1".to_owned(),
            data,
            stroke_width: 2,
            shadow_color: Some("#000".to_owned()),
            corner_radius: None,
        });
    }
}

#[test]
fn the_oracle_rejects_the_externally_tagged_reading() {
    let (ty, types) = described::<ElementData>();
    assert!(!fits(
        &json!({ "text": { "content": "hi", "fontSize": 12, "bold": true } }),
        &ty,
        &types
    ));
    assert!(!fits(&json!("rect"), &ty, &types));
}

type Attrs = BTreeMap<String, Option<String>>;

// mero-drive's `Change`, the `apply_delta` / `title_apply_delta` op: untagged.
#[derive(AbiType, Serialize)]
#[serde(crate = "calimero_sdk::serde", untagged)]
enum Change {
    Retain {
        retain: usize,
        #[serde(default)]
        attributes: Option<Attrs>,
    },
    Insert {
        insert: String,
        #[serde(default)]
        attributes: Option<Attrs>,
    },
    Delete {
        delete: usize,
    },
}

#[test]
fn an_untagged_enum_is_its_bare_payloads() {
    let (_, types) = described::<Change>();
    let map = json!({ "kind": "map", "key": { "kind": "string" }, "value": { "kind": "string" } });
    assert_eq!(
        to_value(&types).unwrap(),
        json!({
            "Change": {
                "kind": "variant",
                "untagged": true,
                "variants": [
                    { "name": "Retain", "payload": { "$ref": "Change_Retain" } },
                    { "name": "Insert", "payload": { "$ref": "Change_Insert" } },
                    { "name": "Delete", "payload": { "$ref": "Change_Delete" } },
                ],
            },
            "Change_Retain": {
                "kind": "record",
                "fields": [
                    { "name": "retain", "type": { "kind": "u32" } },
                    { "name": "attributes", "nullable": true, "type": map },
                ],
            },
            "Change_Insert": {
                "kind": "record",
                "fields": [
                    { "name": "insert", "type": { "kind": "string" } },
                    { "name": "attributes", "nullable": true, "type": map },
                ],
            },
            "Change_Delete": {
                "kind": "record",
                "fields": [{ "name": "delete", "type": { "kind": "u32" } }],
            },
        })
    );
    let bold: Attrs = [("bold".to_owned(), Some("true".to_owned()))].into();
    assert_wire(&Change::Retain {
        retain: 6,
        attributes: Some(bold),
    });
    assert_wire(&Change::Insert {
        insert: "hi".to_owned(),
        attributes: None,
    });
    assert_wire(&Change::Delete { delete: 2 });
}

// mero-drive's `DriveError` tagging: adjacent `kind` / `data`.
#[derive(AbiType, Serialize)]
#[serde(crate = "calimero_sdk::serde", tag = "kind", content = "data")]
enum Outcome {
    NotFound(String),
    Done,
}

#[test]
fn an_adjacently_tagged_enum_names_both_keys() {
    let (_, types) = described::<Outcome>();
    assert_eq!(
        to_value(&types).unwrap(),
        json!({
            "Outcome": {
                "kind": "variant",
                "tag": "kind",
                "content": "data",
                "variants": [
                    { "name": "NotFound", "payload": { "kind": "string" } },
                    { "name": "Done" },
                ],
            }
        })
    );
    assert_wire(&Outcome::NotFound("doc".to_owned()));
    assert_wire(&Outcome::Done);
}

#[derive(AbiType, Serialize)]
#[serde(crate = "calimero_sdk::serde", rename_all_fields = "camelCase")]
enum Moves {
    #[serde(rename = "go")]
    Go { step_size: u32 },
    #[serde(rename_all = "UPPERCASE")]
    Turn { turn_angle: i32 },
    #[serde(skip)]
    Internal,
}

#[test]
fn variant_renames_field_rules_and_skips_apply() {
    let (_, types) = described::<Moves>();
    assert_eq!(
        to_value(&types).unwrap(),
        json!({
            "Moves": {
                "kind": "variant",
                "variants": [
                    { "name": "go", "payload": { "$ref": "Moves_Go" } },
                    { "name": "Turn", "payload": { "$ref": "Moves_Turn" } },
                ],
            },
            "Moves_Go": { "kind": "record", "fields": [{ "name": "stepSize", "type": { "kind": "u32" } }] },
            "Moves_Turn": { "kind": "record", "fields": [{ "name": "TURN_ANGLE", "type": { "kind": "i32" } }] },
        })
    );
    assert_wire(&Moves::Go { step_size: 1 });
    assert_wire(&Moves::Turn { turn_angle: -90 });
}

macro_rules! variant_rule {
    ($name:ident, $rule:tt) => {
        #[derive(AbiType, Serialize)]
        #[serde(crate = "calimero_sdk::serde", rename_all = $rule)]
        enum $name {
            SomeVariant,
        }
    };
}

variant_rule!(VariantLower, "lowercase");
variant_rule!(VariantUpper, "UPPERCASE");
variant_rule!(VariantPascal, "PascalCase");
variant_rule!(VariantCamel, "camelCase");
variant_rule!(VariantSnake, "snake_case");
variant_rule!(VariantScreamingSnake, "SCREAMING_SNAKE_CASE");
variant_rule!(VariantKebab, "kebab-case");
variant_rule!(VariantScreamingKebab, "SCREAMING-KEBAB-CASE");

#[test]
fn every_variant_rename_rule_matches_serde() {
    assert_wire(&VariantLower::SomeVariant);
    assert_wire(&VariantUpper::SomeVariant);
    assert_wire(&VariantPascal::SomeVariant);
    assert_wire(&VariantCamel::SomeVariant);
    assert_wire(&VariantSnake::SomeVariant);
    assert_wire(&VariantScreamingSnake::SomeVariant);
    assert_wire(&VariantKebab::SomeVariant);
    assert_wire(&VariantScreamingKebab::SomeVariant);
}

#[app::event]
#[serde(rename_all = "snake_case")]
pub enum RenamedEvent {
    BlockSet(u32),
    Cleared,
}

#[test]
fn event_names_are_the_kind_serde_emits() {
    let mut reg = TypeRegistry::new();
    let names: Vec<String> = <RenamedEvent as AbiEvents>::abi_events(&mut reg)
        .into_iter()
        .map(|event| event.name)
        .collect();
    assert_eq!(names, ["block_set", "cleared"]);
    assert_eq!(
        to_value(RenamedEvent::BlockSet(1)).unwrap()["kind"],
        "block_set"
    );
}

#[app::event]
pub enum SkippingEvent {
    #[serde(rename_all = "camelCase")]
    Moved { step_size: u32 },
    #[serde(skip)]
    Internal,
}

#[test]
fn event_variants_honour_serde_skips_and_field_rules() {
    let mut reg = TypeRegistry::new();
    let events = <SkippingEvent as AbiEvents>::abi_events(&mut reg);
    assert_eq!(
        to_value(events).unwrap(),
        json!([{ "name": "Moved", "payload": { "$ref": "SkippingEvent_Moved" } }])
    );
    assert_eq!(
        to_value(reg.into_types()).unwrap(),
        json!({
            "SkippingEvent_Moved": {
                "kind": "record",
                "fields": [{ "name": "stepSize", "type": { "kind": "u32" } }],
            }
        })
    );
    assert_eq!(
        to_value(SkippingEvent::Moved { step_size: 1 }).unwrap()["data"],
        json!({ "stepSize": 1 })
    );
}

#[derive(AbiType, Serialize)]
#[serde(crate = "calimero_sdk::serde")]
struct Point(i32, i32);

#[derive(AbiType, Serialize)]
#[serde(crate = "calimero_sdk::serde", tag = "kind", content = "data")]
enum Motion {
    Step(i32, i32),
    Stop,
}

#[test]
fn positional_values_are_tuples() {
    let (_, types) = described::<Point>();
    assert_eq!(
        to_value(&types).unwrap(),
        json!({
            "Point": {
                "kind": "alias",
                "target": { "kind": "tuple", "elements": [{ "kind": "i32" }, { "kind": "i32" }] },
            }
        })
    );
    assert_wire(&Point(3, -4));
    assert_wire(&Motion::Step(1, 2));
    assert_wire(&Motion::Stop);
}

#[derive(AbiType, Serialize)]
#[serde(crate = "calimero_sdk::serde")]
struct Stamped(u32, #[serde(skip)] String, bool);

#[test]
fn a_skipped_positional_field_leaves_the_tuple() {
    let (_, types) = described::<Stamped>();
    assert_eq!(
        to_value(&types).unwrap()["Stamped"]["target"]["elements"],
        json!([{ "kind": "u32" }, { "kind": "bool" }])
    );
    assert_wire(&Stamped(1, String::new(), true));
}
