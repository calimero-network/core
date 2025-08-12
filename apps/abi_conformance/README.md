# ABI Conformance Application

A Calimero SSApp designed to exercise the WASM-ABI v1 generator with a comprehensive set of types, methods, errors, and events.

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

## Canonical Types

The ABI uses the following canonical WASM types:

- **Scalar types**: `bool`, `i32`, `i64`, `u32`, `u64`, `string`, `bytes`
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

## Complex Types

The application defines several complex types that are referenced in the ABI:

- `UserId`: Newtype bytes wrapper for user identification
- `Person`: Record structure with user information
- `Action`: Variant enum with different action types
- `ConformanceError`: Error enum for method-level errors
- `AbiState`: State record with maps and lists

## Methods

The application exposes a comprehensive set of public methods:

- **Scalars**: `echo_scalars` - demonstrates basic scalar types
- **Optionals**: `opt_number` - demonstrates nullable types
- **Lists**: `sum_i64` - demonstrates list operations
- **Maps**: `score_of` - demonstrates map operations with string keys
- **Records**: `make_person` - demonstrates record type handling
- **Variants**: `act` - demonstrates variant enum handling
- **Bytes**: `roundtrip_id` - demonstrates bytes newtype handling
- **Errors**: `may_fail`, `may_fail_not_found` - demonstrate error handling

## Events

The application defines events with various payload types:

- `Ping`: Unit event (no payload)
- `Named`: String payload
- `Data`: Bytes payload
- `Updated`: Record payload

## ABI Conformance

This application serves as a test of the WASM-ABI v1 generator. Currently, the ABI extraction captures:

1. Basic scalar types (bool, i32, i64, u32, u64, string)
2. Bytes newtype (UserId as bytes with hex encoding)
3. Basic method signatures and return types
4. Some variant types (Error enum)

### Current Limitations

The ABI extraction has some limitations that are being worked on:

1. **Complex types**: Records (Person) and variants (Action) are referenced but not fully expanded in the types section
2. **Collections**: Vec<T>, Option<T>, and BTreeMap<K,V> are currently serialized as strings instead of their proper representations
3. **Events**: Event definitions are not currently captured in the ABI
4. **Method errors**: Error types for methods are not currently captured
5. **Nullable types**: Option<T> is not represented as nullable fields

### Future Improvements

The ABI extraction tool is being enhanced to properly handle:
- Full type expansion for records and variants
- Proper collection type representations
- Event capture and serialization
- Method-level error definitions
- Nullable type handling

This app serves as a baseline for testing these improvements as they are implemented. 