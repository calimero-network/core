use calimero_wasm_abi_v1::normalize::{
    normalize_type, NormalizeError, ResolvedLocal, TypeResolver,
};
use calimero_wasm_abi_v1::schema::TypeRef;
use syn::parse_str;

// Mock resolver for testing
struct MockResolver {
    locals: std::collections::HashMap<String, ResolvedLocal>,
}

impl MockResolver {
    fn new() -> Self {
        Self {
            locals: std::collections::HashMap::new(),
        }
    }

    fn add_newtype_bytes(&mut self, name: &str, size: usize) {
        self.locals
            .insert(name.to_string(), ResolvedLocal::NewtypeBytes { size });
    }

    fn add_record(&mut self, name: &str) {
        self.locals.insert(name.to_string(), ResolvedLocal::Record);
    }

    fn add_variant(&mut self, name: &str) {
        self.locals.insert(name.to_string(), ResolvedLocal::Variant);
    }
}

impl TypeResolver for MockResolver {
    fn resolve_local(&self, path: &str) -> Option<ResolvedLocal> {
        self.locals.get(path).cloned()
    }
}

fn parse_type(ty_str: &str) -> syn::Type {
    parse_str(ty_str).expect("failed to parse type")
}

#[test]
fn test_scalar_types() {
    let resolver = MockResolver::new();

    // Basic scalar types
    assert_eq!(
        normalize_type(&parse_type("bool"), true, &resolver).unwrap(),
        TypeRef::bool()
    );
    assert_eq!(
        normalize_type(&parse_type("i32"), true, &resolver).unwrap(),
        TypeRef::i32()
    );
    assert_eq!(
        normalize_type(&parse_type("i64"), true, &resolver).unwrap(),
        TypeRef::i64()
    );
    assert_eq!(
        normalize_type(&parse_type("u32"), true, &resolver).unwrap(),
        TypeRef::u32()
    );
    assert_eq!(
        normalize_type(&parse_type("u64"), true, &resolver).unwrap(),
        TypeRef::u64()
    );
    assert_eq!(
        normalize_type(&parse_type("f32"), true, &resolver).unwrap(),
        TypeRef::f32()
    );
    assert_eq!(
        normalize_type(&parse_type("f64"), true, &resolver).unwrap(),
        TypeRef::f64()
    );
    assert_eq!(
        normalize_type(&parse_type("String"), true, &resolver).unwrap(),
        TypeRef::string()
    );
    assert_eq!(
        normalize_type(&parse_type("str"), true, &resolver).unwrap(),
        TypeRef::string()
    );
}

#[test]
fn test_wasm32_size_mapping() {
    let resolver = MockResolver::new();

    // On wasm32, usize/isize map to u32/i32
    assert_eq!(
        normalize_type(&parse_type("usize"), true, &resolver).unwrap(),
        TypeRef::u32()
    );
    assert_eq!(
        normalize_type(&parse_type("isize"), true, &resolver).unwrap(),
        TypeRef::i32()
    );

    // On non-wasm32, they map to u64/i64
    assert_eq!(
        normalize_type(&parse_type("usize"), false, &resolver).unwrap(),
        TypeRef::u64()
    );
    assert_eq!(
        normalize_type(&parse_type("isize"), false, &resolver).unwrap(),
        TypeRef::i64()
    );
}

#[test]
fn test_references_and_lifetimes() {
    let resolver = MockResolver::new();

    // &str -> string
    assert_eq!(
        normalize_type(&parse_type("&str"), true, &resolver).unwrap(),
        TypeRef::string()
    );

    // &'a str -> string
    assert_eq!(
        normalize_type(&parse_type("&'a str"), true, &resolver).unwrap(),
        TypeRef::string()
    );

    // &T -> T (for named types)
    let mut resolver = MockResolver::new();
    resolver.add_record("Person");
    assert_eq!(
        normalize_type(&parse_type("&Person"), true, &resolver).unwrap(),
        TypeRef::reference("Person")
    );
}

#[test]
fn test_option_types() {
    let resolver = MockResolver::new();

    // Option<u32> -> u32 with nullable
    let result = normalize_type(&parse_type("Option<u32>"), true, &resolver).unwrap();
    assert_eq!(result, TypeRef::u32());
    // Note: nullable is handled at the Parameter/Field level, not in TypeRef itself

    // Option<String> -> string with nullable
    let result = normalize_type(&parse_type("Option<String>"), true, &resolver).unwrap();
    assert_eq!(result, TypeRef::string());

    // Option<Person> -> Person reference with nullable
    let mut resolver = MockResolver::new();
    resolver.add_record("Person");
    let result = normalize_type(&parse_type("Option<Person>"), true, &resolver).unwrap();
    assert_eq!(result, TypeRef::reference("Person"));
}

#[test]
fn test_vec_types() {
    let resolver = MockResolver::new();

    // Vec<u32> -> list<u32>
    let result = normalize_type(&parse_type("Vec<u32>"), true, &resolver).unwrap();
    assert_eq!(result, TypeRef::list(TypeRef::u32()));

    // Vec<String> -> list<string>
    let result = normalize_type(&parse_type("Vec<String>"), true, &resolver).unwrap();
    assert_eq!(result, TypeRef::list(TypeRef::string()));

    // Vec<Person> -> list<Person>
    let mut resolver = MockResolver::new();
    resolver.add_record("Person");
    let result = normalize_type(&parse_type("Vec<Person>"), true, &resolver).unwrap();
    assert_eq!(result, TypeRef::list(TypeRef::reference("Person")));
}

#[test]
fn test_btree_map_types() {
    let resolver = MockResolver::new();

    // BTreeMap<String, u32> -> map<string, u32>
    let result = normalize_type(&parse_type("BTreeMap<String, u32>"), true, &resolver).unwrap();
    assert_eq!(result, TypeRef::map(TypeRef::u32()));

    // BTreeMap<String, String> -> map<string, string>
    let result = normalize_type(&parse_type("BTreeMap<String, String>"), true, &resolver).unwrap();
    assert_eq!(result, TypeRef::map(TypeRef::string()));

    // BTreeMap<String, Person> -> map<string, Person>
    let mut resolver = MockResolver::new();
    resolver.add_record("Person");
    let result = normalize_type(&parse_type("BTreeMap<String, Person>"), true, &resolver).unwrap();
    assert_eq!(result, TypeRef::map(TypeRef::reference("Person")));
}

#[test]
fn test_btree_map_invalid_keys() {
    let resolver = MockResolver::new();

    // BTreeMap<u32, String> should fail
    let result = normalize_type(&parse_type("BTreeMap<u32, String>"), true, &resolver);
    assert!(matches!(result, Err(NormalizeError::UnsupportedMapKey(_))));

    // BTreeMap<i32, String> should fail
    let result = normalize_type(&parse_type("BTreeMap<i32, String>"), true, &resolver);
    assert!(matches!(result, Err(NormalizeError::UnsupportedMapKey(_))));
}

#[test]
fn test_array_types() {
    let resolver = MockResolver::new();

    // [u8; 32] -> bytes{size:32}
    let result = normalize_type(&parse_type("[u8; 32]"), true, &resolver).unwrap();
    assert_eq!(result, TypeRef::bytes_with_size(32, "hex"));

    // [u8; 64] -> bytes{size:64}
    let result = normalize_type(&parse_type("[u8; 64]"), true, &resolver).unwrap();
    assert_eq!(result, TypeRef::bytes_with_size(64, "hex"));
}

#[test]
fn test_array_invalid_elements() {
    let resolver = MockResolver::new();

    // [u32; 10] should fail
    let result = normalize_type(&parse_type("[u32; 10]"), true, &resolver);
    assert!(matches!(
        result,
        Err(NormalizeError::UnsupportedArrayElement(_))
    ));

    // [String; 5] should fail
    let result = normalize_type(&parse_type("[String; 5]"), true, &resolver);
    assert!(matches!(
        result,
        Err(NormalizeError::UnsupportedArrayElement(_))
    ));
}

#[test]
fn test_vec_u8_bytes() {
    let resolver = MockResolver::new();

    // Vec<u8> -> bytes (no size)
    let result = normalize_type(&parse_type("Vec<u8>"), true, &resolver).unwrap();
    assert_eq!(result, TypeRef::bytes());
}

#[test]
fn test_newtype_bytes() {
    let mut resolver = MockResolver::new();
    resolver.add_newtype_bytes("UserId32", 32);
    resolver.add_newtype_bytes("Hash64", 64);

    // UserId32 -> bytes{size:32} (not a reference)
    let result = normalize_type(&parse_type("UserId32"), true, &resolver).unwrap();
    assert_eq!(result, TypeRef::bytes_with_size(32, "hex"));

    // Hash64 -> bytes{size:64} (not a reference)
    let result = normalize_type(&parse_type("Hash64"), true, &resolver).unwrap();
    assert_eq!(result, TypeRef::bytes_with_size(64, "hex"));
}

#[test]
fn test_record_and_variant_types() {
    let mut resolver = MockResolver::new();
    resolver.add_record("Person");
    resolver.add_variant("Action");

    // Person -> $ref:"Person"
    let result = normalize_type(&parse_type("Person"), true, &resolver).unwrap();
    assert_eq!(result, TypeRef::reference("Person"));

    // Action -> $ref:"Action"
    let result = normalize_type(&parse_type("Action"), true, &resolver).unwrap();
    assert_eq!(result, TypeRef::reference("Action"));
}

#[test]
fn test_unknown_external_types() {
    let resolver = MockResolver::new();

    // Unknown types should fail with TypePathError
    assert!(normalize_type(&parse_type("ExternalType"), true, &resolver).is_err());

    // Fully qualified paths should also fail
    assert!(normalize_type(&parse_type("std::collections::HashMap"), true, &resolver).is_err());
}

#[test]
fn test_unit_type() {
    let resolver = MockResolver::new();

    // () -> unit
    let result = normalize_type(&parse_type("()"), true, &resolver).unwrap();
    assert_eq!(result, TypeRef::unit());
}

#[test]
fn test_nested_generics() {
    let mut resolver = MockResolver::new();
    resolver.add_newtype_bytes("UserId32", 32);
    resolver.add_record("Person");

    // Option<Vec<u32>> -> list<u32> with nullable
    let result = normalize_type(&parse_type("Option<Vec<u32>>"), true, &resolver).unwrap();
    assert_eq!(result, TypeRef::list(TypeRef::u32()));

    // Option<Vec<UserId32>> -> list<bytes{size:32}> with nullable
    let result = normalize_type(&parse_type("Option<Vec<UserId32>>"), true, &resolver).unwrap();
    assert_eq!(result, TypeRef::list(TypeRef::bytes_with_size(32, "hex")));

    // Vec<Option<Person>> -> list<Person> (nullable handled at field level)
    let result = normalize_type(&parse_type("Vec<Option<Person>>"), true, &resolver).unwrap();
    assert_eq!(result, TypeRef::list(TypeRef::reference("Person")));

    // BTreeMap<String, Vec<u32>> -> map<string, list<u32>>
    let result =
        normalize_type(&parse_type("BTreeMap<String, Vec<u32>>"), true, &resolver).unwrap();
    assert_eq!(result, TypeRef::map(TypeRef::list(TypeRef::u32())));
}

#[test]
fn test_complex_nested_scenarios() {
    let mut resolver = MockResolver::new();
    resolver.add_newtype_bytes("UserId32", 32);
    resolver.add_record("Person");

    // Option<BTreeMap<String, Vec<Person>>> -> map<string, list<Person>> with nullable
    let result = normalize_type(
        &parse_type("Option<BTreeMap<String, Vec<Person>>>"),
        true,
        &resolver,
    )
    .unwrap();
    assert_eq!(
        result,
        TypeRef::map(TypeRef::list(TypeRef::reference("Person")))
    );

    // Vec<Option<BTreeMap<String, UserId32>>> -> list<map<string, bytes{size:32}>>
    let result = normalize_type(
        &parse_type("Vec<Option<BTreeMap<String, UserId32>>>"),
        true,
        &resolver,
    )
    .unwrap();
    assert_eq!(
        result,
        TypeRef::list(TypeRef::map(TypeRef::bytes_with_size(32, "hex")))
    );
}
