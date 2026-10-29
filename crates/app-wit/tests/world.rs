use std::collections::BTreeSet;

use calimero_app_wit::{allowed_imports, stubbed_imports, WIT_DIR, WORLD};
use wit_parser::{Resolve, WorldItem};

fn qualified(resolve: &Resolve, id: wit_parser::InterfaceId) -> String {
    let iface = &resolve.interfaces[id];
    let package = &resolve.packages[iface.package.expect("interface has a package")].name;
    let version = package
        .version
        .as_ref()
        .map(|v| format!("@{v}"))
        .unwrap_or_default();
    format!(
        "{}:{}/{}{}",
        package.namespace,
        package.name,
        iface.name.as_deref().expect("named interface"),
        version
    )
}

#[test]
fn world_parses_and_imports_exactly_the_spec_set() {
    let mut resolve = Resolve::default();
    let (pkg, _) = resolve.push_dir(WIT_DIR).expect("wit dir parses");
    let world_id = resolve
        .select_world(&[pkg], Some("app"))
        .expect("world app exists");
    let world = &resolve.worlds[world_id];
    assert_eq!(WORLD, "calimero:app/app@0.5.1");

    let mut imports = BTreeSet::new();
    for item in world.imports.values() {
        match item {
            WorldItem::Interface { id, .. } => {
                imports.insert(qualified(&resolve, *id));
            }
            // `use types.{..}` in the world surfaces as imported types, not interfaces.
            WorldItem::Type { .. } => {}
            other => panic!("unexpected import: {other:?}"),
        }
    }
    let expected: BTreeSet<String> = allowed_imports().iter().map(|s| s.to_string()).collect();
    assert_eq!(imports, expected);
    assert_eq!(allowed_imports().len(), 21);

    let exports: Vec<_> = world.exports.keys().map(|k| format!("{k:?}")).collect();
    assert_eq!(exports.len(), 1, "exports: {exports:?}");
    let (_, item) = world.exports.iter().next().unwrap();
    match item {
        WorldItem::Function(f) => assert_eq!(f.name, "handle"),
        other => panic!("export is not a function: {other:?}"),
    }
}

#[test]
fn stubbed_imports_are_nine_disjoint_and_not_sockets() {
    let stubbed = stubbed_imports();
    assert_eq!(stubbed.len(), 9);
    assert!(stubbed.iter().all(|s| !s.starts_with("wasi:sockets/")));
    assert!(stubbed.iter().all(|s| s.ends_with("@0.2.12")));
    let allowed: BTreeSet<_> = allowed_imports().iter().collect();
    assert!(stubbed.iter().all(|s| !allowed.contains(s)));
}
