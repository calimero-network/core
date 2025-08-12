# ABI Conformance Application

A Calimero SSApp designed to exercise the WASM-ABI v1 generator with a comprehensive set of types, methods, errors, and events. This application provides **exhaustive coverage** of all canonical WASM-ABI v1 kinds as both inputs and outputs.

## Building

To build the application for WASM:

```bash
# Using the build script (recommended)
cd apps/abi_conformance
./build.sh

# Or using cargo directly
rustup target add wasm32-unknown-unknown
cargo build -p abi_conformance --target wasm32-unknown-unknown
```

The build script will:
1. Add the wasm32-unknown-unknown target
2. Build the app with release optimizations
3. Copy the WASM file to `res/abi_conformance.wasm`
4. Optimize the WASM file with wasm-opt if available

## ABI Extraction

To extract the ABI from the compiled WASM:

```bash
# Build the calimero-abi tool first
cargo build -p calimero-abi

# Extract ABI from the optimized WASM file
./target/debug/calimero-abi extract apps/abi_conformance/res/abi_conformance.wasm -o apps/abi_conformance/res/abi.json

# Or extract from debug build
./target/debug/calimero-abi extract target/wasm32-unknown-unknown/debug/abi_conformance.wasm -o /tmp/abi.json
```

## ABI Verification

To verify the extracted ABI matches the expected golden ABI:

```bash
diff -u apps/abi_conformance/abi.expected.json /tmp/abi.json
```

## Exhaustive Coverage Matrix

This application provides **complete coverage** of all canonical WASM-ABI v1 kinds. Each kind appears as both a **parameter** and a **return value** in at least one method:

| Kind | Parameter | Return | Methods |
|------|-----------|--------|---------|
| **Scalars** |
| `bool` | ✓ | ✓ | `echo_bool` |
| `i32` | ✓ | ✓ | `echo_i32` |
| `i64` | ✓ | ✓ | `echo_i64` |
| `u32` | ✓ | ✓ | `echo_u32` |
| `u64` | ✓ | ✓ | `echo_u64` |
| `f32` | ✓ | ✓ | `echo_f32` |
| `f64` | ✓ | ✓ | `echo_f64` |
| `string` | ✓ | ✓ | `echo_string` |
| `bytes` | ✓ | ✓ | `echo_bytes` |
| **Unit** |
| `unit` | ✓ | ✓ | `noop` |
| **Optionals** |
| `Option<u32>` | ✓ | ✓ | `opt_u32` |
| `Option<string>` | ✓ | ✓ | `opt_string` |
| `Option<Person>` | ✓ | ✓ | `opt_record` |
| `Option<UserId32>` | ✓ | ✓ | `opt_id` |
| **Lists** |
| `list<u32>` | ✓ | ✓ | `list_u32` |
| `list<string>` | ✓ | ✓ | `list_strings` |
| `list<Person>` | ✓ | ✓ | `list_records` |
| `list<UserId32>` | ✓ | ✓ | `list_ids` |
| **Maps** |
| `map<string,u32>` | ✓ | ✓ | `map_u32` |
| `map<string,list<u32>>` | ✓ | ✓ | `map_list_u32` |
| `map<string,Person>` | ✓ | ✓ | `map_record` |
| **Records** |
| `Person` | ✓ | ✓ | `make_person` |
| `Profile` | ✓ | ✓ | `profile_roundtrip` |
| `AbiState` | ✓ | ✓ | `init` |
| **Variants** |
| `Action` | ✓ | ✓ | `act` |
| `ConformanceError` | ✓ | ✓ | `may_fail`, `find_person` |
| **Bytes Newtypes** |
| `UserId32` (32 bytes) | ✓ | ✓ | `roundtrip_id` |
| `Hash64` (64 bytes) | ✓ | ✓ | `roundtrip_hash` |

## Canonical Types

The ABI uses the following canonical WASM types:

- **Scalar types**: `bool`, `i32`, `i64`, `u32`, `u64`, `f32`, `f64`, `string`, `bytes`
- **Collection types**: `list<T>`, `map<string,V>`
- **Nullable types**: `Option<T>` is represented as nullable `T`
- **Result types**: `Result<T,E>` is normalized to return `T` with errors handled separately

## Type Normalization Rules

- `usize`/`isize` → `u32`/`i32` (wasm32)
- `&str` → `string`
- `Vec<T>` → `list<T>`
- `Option<T>` → nullable `T`
- `Result<T,E>` → `T` (error handling separate)
- Custom types → `$ref` to type name
- `[u8; N]` → `bytes` with `size: N` and `encoding: "hex"`

## Complex Types

The application defines several complex types that are referenced in the ABI:

- `UserId32`: Newtype bytes wrapper for user identification (32 bytes)
- `Hash64`: Newtype bytes wrapper for hashes (64 bytes)
- `Person`: Record structure with user information
- `Profile`: Record structure with optional fields and lists
- `Action`: Variant enum with different action types
- `ConformanceError`: Error enum for method-level errors
- `AbiState`: State record with maps and lists

## Methods

The application exposes a comprehensive set of public methods covering all canonical kinds:

### Unit
- `noop() -> ()` - Unit return demonstration

### Scalars
- `echo_bool(b: bool) -> bool`
- `echo_i32(v: i32) -> i32`
- `echo_i64(v: i64) -> i64`
- `echo_u32(v: u32) -> u32`
- `echo_u64(v: u64) -> u64`
- `echo_f32(v: f32) -> f32`
- `echo_f64(v: f64) -> f64`
- `echo_string(s: String) -> String`
- `echo_bytes(b: Vec<u8>) -> Vec<u8>`

### Optionals
- `opt_u32(x: Option<u32>) -> Option<u32>`
- `opt_string(x: Option<String>) -> Option<String>`
- `opt_record(p: Option<Person>) -> Option<Person>`
- `opt_id(x: Option<UserId32>) -> Option<UserId32>`

### Lists
- `list_u32(xs: Vec<u32>) -> Vec<u32>`
- `list_strings(xs: Vec<String>) -> Vec<String>`
- `list_records(ps: Vec<Person>) -> Vec<Person>`
- `list_ids(xs: Vec<UserId32>) -> Vec<UserId32>`

### Maps
- `map_u32(m: BTreeMap<String, u32>) -> BTreeMap<String, u32>`
- `map_list_u32(m: BTreeMap<String, Vec<u32>>) -> BTreeMap<String, Vec<u32>>`
- `map_record(m: BTreeMap<String, Person>) -> BTreeMap<String, Person>`

### Records
- `make_person(p: Person) -> Person`
- `profile_roundtrip(p: Profile) -> Profile`

### Variants
- `act(a: Action) -> u32`

### Bytes Newtypes
- `roundtrip_id(x: UserId32) -> UserId32`
- `roundtrip_hash(h: Hash64) -> Hash64`

### Errors
- `may_fail(flag: bool) -> Result<u32, ConformanceError>`
- `find_person(name: String) -> Result<Person, ConformanceError>`

### Init
- `init() -> AbiState` - Returns state record with maps and lists

## Events

The application defines events with various payload types:

- `Ping`: Unit event (no payload)
- `Named`: String payload
- `Data`: Bytes payload
- `PersonUpdated`: Record payload
- `ActionTaken`: Variant payload

## ABI Conformance

This application serves as a comprehensive test of the WASM-ABI v1 generator. The ABI extraction captures:

1. **All scalar types** (bool, i32, i64, u32, u64, f32, f64, string, bytes)
2. **Unit type** for void returns
3. **Nullable types** (Option<T> with nullable: true)
4. **List types** (Vec<T> as list<T>)
5. **Map types** (BTreeMap<String, V> as map<string,V>)
6. **Record types** (structs with fields)
7. **Variant types** (enums with variants)
8. **Bytes newtypes** (UserId32, Hash64 as bytes with hex encoding)
9. **Method errors** (Result<T,E> with separate errors array)
10. **Events** with various payload types

### Current Status

The ABI extraction tool is being enhanced to properly handle all these types. Currently, some complex types may be serialized as strings instead of their proper representations. The `abi.expected.json` file contains the canonical expected output that the tool should produce.

### Future Improvements

The ABI extraction tool is being enhanced to properly handle:
- Full type expansion for records and variants
- Proper collection type representations (lists, maps)
- Event capture and serialization
- Method-level error definitions
- Nullable type handling
- Bytes newtype handling

This app serves as a baseline for testing these improvements as they are implemented. 