use std::collections::BTreeSet;

use calimero_app_wit::{
    allowed_imports, compute_allowed_imports, stubbed_imports, COMPUTE_WORLD, WIT_DIR, WORLD,
};
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
    assert_eq!(allowed_imports().len(), 24);

    let exports: Vec<_> = world.exports.keys().map(|k| format!("{k:?}")).collect();
    assert_eq!(exports.len(), 1, "exports: {exports:?}");
    let (_, item) = world.exports.iter().next().unwrap();
    match item {
        WorldItem::Function(f) => assert_eq!(f.name, "handle"),
        other => panic!("export is not a function: {other:?}"),
    }
}

fn imports_of_world(name: &str) -> BTreeSet<String> {
    let mut resolve = Resolve::default();
    let (pkg, _) = resolve.push_dir(WIT_DIR).expect("wit dir parses");
    let world_id = resolve
        .select_world(&[pkg], Some(name))
        .unwrap_or_else(|e| panic!("world {name}: {e}"));
    resolve.worlds[world_id]
        .imports
        .values()
        .filter_map(|item| match item {
            WorldItem::Interface { id, .. } => Some(qualified(&resolve, *id)),
            _ => None,
        })
        .collect()
}

#[test]
fn compute_world_imports_exactly_its_allow_list_and_exports_only_run() {
    assert_eq!(COMPUTE_WORLD, "calimero:app/compute@0.5.1");
    let expected: BTreeSet<String> = compute_allowed_imports()
        .iter()
        .map(|s| s.to_string())
        .collect();
    assert_eq!(imports_of_world("compute"), expected);

    let mut resolve = Resolve::default();
    let (pkg, _) = resolve.push_dir(WIT_DIR).expect("wit dir parses");
    let world = &resolve.worlds[resolve.select_world(&[pkg], Some("compute")).unwrap()];
    let exports: Vec<_> = world.exports.values().collect();
    assert_eq!(exports.len(), 1);
    match exports[0] {
        WorldItem::Function(f) => assert_eq!(f.name, "run"),
        other => panic!("export is not a function: {other:?}"),
    }
}

#[test]
fn compute_world_has_no_state_xcall_or_http() {
    let imports = imports_of_world("compute");
    for name in [
        "storage", "xcall", "context", "events", "action", "search", "runs",
    ] {
        assert!(
            !imports.contains(&format!("calimero:app/{name}@0.5.1")),
            "{name}"
        );
    }
    assert!(imports.iter().all(|i| !i.starts_with("wasi:http/")));
    assert!(imports.contains("calimero:app/job@0.5.1"));
}

#[test]
fn app_world_does_not_import_job() {
    assert!(!imports_of_world("app").contains("calimero:app/job@0.5.1"));
}

#[test]
fn stubbed_imports_are_disjoint_and_include_every_sockets_interface() {
    let stubbed = stubbed_imports();
    assert_eq!(stubbed.len(), 16);
    // componentize-py 0.25.1's default build imports all seven (Xabi, 2026-11-09).
    for interface in [
        "instance-network",
        "ip-name-lookup",
        "network",
        "tcp-create-socket",
        "tcp",
        "udp-create-socket",
        "udp",
    ] {
        let import = format!("wasi:sockets/{interface}@0.2.12");
        assert!(stubbed.contains(&import.as_str()), "{import}");
    }
    assert!(stubbed.iter().all(|s| s.ends_with("@0.2.12")));
    let allowed: BTreeSet<_> = allowed_imports().iter().collect();
    assert!(stubbed.iter().all(|s| !allowed.contains(s)));
}

#[test]
fn runs_uploader_takes_a_blob_id_and_answers_an_optional_account_and_files_has_none() {
    let mut resolve = Resolve::default();
    let (pkg, _) = resolve.push_dir(WIT_DIR).expect("wit dir parses");
    let interfaces = &resolve.packages[pkg].interfaces;
    let uploader = resolve.interfaces[interfaces["runs"]]
        .functions
        .get("uploader")
        .expect("runs.uploader");
    assert_eq!(uploader.params.len(), 1);
    assert_eq!(uploader.params[0].name, "id");
    assert!(uploader.result.is_some());
    let files = &resolve.interfaces[interfaces["files"]].functions;
    assert!(
        !files.contains_key("uploader"),
        "the compute world imports files"
    );
}

#[test]
fn runs_publish_takes_a_blob_id_and_answers_a_blob_id_or_a_string() {
    let mut resolve = Resolve::default();
    let (pkg, _) = resolve.push_dir(WIT_DIR).expect("wit dir parses");
    let runs = resolve.packages[pkg]
        .interfaces
        .get("runs")
        .expect("interface runs");
    let publish = resolve.interfaces[*runs]
        .functions
        .get("publish")
        .expect("runs.publish");
    assert_eq!(publish.params.len(), 1);
    assert_eq!(publish.params[0].name, "id");
    assert!(publish.result.is_some());
}
