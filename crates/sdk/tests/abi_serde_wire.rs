#![allow(
    dead_code,
    reason = "The corpus types are described and serialized, never read"
)]
//! The ABI describes what serde writes. Each case serializes a real value and
//! checks it against the manifest with `fits`, a reference reading of the ABI's
//! JSON wire rules, so the description cannot drift from serde.

use std::collections::BTreeMap;

use calimero_sdk::abi::{
    AbiType, CollectionType, Field, ScalarType, TypeDef, TypeRef, TypeRegistry, Variant,
};
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
