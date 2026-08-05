//! Locks the invariant that every `Mergeable` implementor also implements
//! `AbiType`: anything Mergeable can sit in app state, and one state field
//! without `AbiType` blocks the app's entire type-system ABI derivation.
//!
//! Two sides: the source scan below finds every `impl ... Mergeable for X`
//! in this crate (completeness), and each `covered::<X>()` call compiles
//! only if the `AbiType` impl actually exists (the guarantee). Adding a new
//! Mergeable wrapper fails this test until it gets an `AbiType` impl too.

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use calimero_storage::collections::{
    AccessControl, AuthoredMap, AuthoredVector, Counter, FrozenStorage, FrozenValue, LwwRegister,
    ReplicatedGrowableArray, SharedStorage, SortedMap, SortedSet, UnorderedMap, UnorderedSet,
    UserStorage, Vector, WriterSetCell,
};
use calimero_wasm_abi::abi_type::AbiType;

/// Compiles only when `T: AbiType` - the actual guarantee.
fn covered<T: AbiType>() {}

/// Mergeable implementors that deliberately have no `AbiType` impl:
/// `#[cfg(test)]` fixtures that can never appear in real app state.
const TEST_ONLY: &[&str] = &["DispatchTestApp", "MyState", "TestState", "TestVal"];

#[test]
fn every_mergeable_implementor_has_an_abi_type_impl() {
    // One instantiation per production implementor; type arguments are
    // arbitrary AbiType-satisfying picks. `SharedStorage` stands in for
    // `PermissionedStorage` (its public alias).
    covered::<AccessControl>();
    covered::<AuthoredMap<String, u64>>();
    covered::<AuthoredVector<u64>>();
    covered::<Box<u64>>();
    covered::<Counter>();
    covered::<FrozenStorage<u64>>();
    covered::<FrozenValue<u64>>();
    covered::<LwwRegister<u64>>();
    covered::<Option<u64>>();
    covered::<ReplicatedGrowableArray>();
    covered::<SharedStorage<LwwRegister<String>>>();
    covered::<SortedMap<String, u64>>();
    covered::<SortedSet<u64>>();
    covered::<UnorderedMap<String, u64>>();
    covered::<UnorderedSet<u64>>();
    covered::<UserStorage<u64>>();
    covered::<Vector<u64>>();
    covered::<WriterSetCell<LwwRegister<String>>>();

    let declared: BTreeSet<&str> = [
        "AccessControl",
        "AuthoredMap",
        "AuthoredVector",
        "Box",
        "Counter",
        "FrozenStorage",
        "FrozenValue",
        "LwwRegister",
        "Option",
        "PermissionedStorage",
        "ReplicatedGrowableArray",
        "SortedMap",
        "SortedSet",
        "UnorderedMap",
        "UnorderedSet",
        "UserStorage",
        "Vector",
        "WriterSetCell",
    ]
    .into_iter()
    .collect();

    let mut found = BTreeSet::new();
    scan(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("src").as_path(),
        &mut found,
    );
    let test_only: BTreeSet<&str> = TEST_ONLY.iter().copied().collect();

    for name in &found {
        assert!(
            declared.contains(name.as_str()) || test_only.contains(name.as_str()),
            "`{name}` implements Mergeable but is not asserted AbiType-covered: add \
             `covered::<{name}<..>>()` and `\"{name}\"` above (the call only compiles once \
             the AbiType impl exists), or add it to TEST_ONLY if it is a #[cfg(test)] fixture"
        );
    }
    for name in &declared {
        assert!(
            found.contains(*name),
            "`{name}` is declared covered but no `impl Mergeable for {name}` was found: \
             remove it from this test"
        );
    }
}

/// Collect the target type name of every `impl ... Mergeable for X` under
/// `dir`. Textual on purpose: macro-generated impls for app types (e.g.
/// `impl_atomic_lww_leaf!`) are the app's responsibility, not this crate's.
fn scan(dir: &Path, out: &mut BTreeSet<String>) {
    for entry in fs::read_dir(dir).expect("readable source dir") {
        let path = entry.expect("readable dir entry").path();
        if path.is_dir() {
            // src/tests/ holds fixtures that never reach real app state.
            if path.file_name().is_some_and(|n| n == "tests") {
                continue;
            }
            scan(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            let text = fs::read_to_string(&path).expect("readable source file");
            for line in text.lines() {
                let line = line.trim_start();
                // Only real impl lines; comments and prose don't count.
                if !line.starts_with("impl") {
                    continue;
                }
                let Some(i) = line.find("Mergeable for ") else {
                    continue;
                };
                let rest = &line[i + "Mergeable for ".len()..];
                let name: String = rest
                    .chars()
                    .take_while(|c| c.is_alphanumeric() || *c == '_')
                    .collect();
                // `$t` etc.: macro definitions, not impls of concrete types.
                if !name.is_empty() {
                    out.insert(name);
                }
            }
        }
    }
}
