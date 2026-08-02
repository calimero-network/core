//! Generated `__calimero_abi()` code assembles a [`Manifest`] through this
//! builder instead of open-coding sorting and defaulting, keeping codegen
//! output small and the sorting rule beside `validate_manifest`.

use crate::abi_type::TypeRegistry;
use crate::schema::{Event, Manifest, Method, MigrationEdgeAbi};

/// Accumulates a manifest's parts as generated code registers types, methods,
/// and events; [`finish`](Self::finish) applies the sorting `validate_manifest`
/// requires.
#[derive(Debug, Default)]
pub struct ManifestBuilder {
    registry: TypeRegistry,
    methods: Vec<Method>,
    events: Vec<Event>,
    state_root: Option<String>,
    state_version: Option<u32>,
    migrations: Vec<MigrationEdgeAbi>,
}

impl ManifestBuilder {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn registry(&mut self) -> &mut TypeRegistry {
        &mut self.registry
    }

    pub fn method(&mut self, m: Method) {
        self.methods.push(m);
    }

    pub fn event(&mut self, e: Event) {
        self.events.push(e);
    }

    pub fn state_root(&mut self, name: &str) {
        self.state_root = Some(name.to_owned());
    }

    pub fn state_version(&mut self, v: u32) {
        self.state_version = Some(v);
    }

    pub fn migration(&mut self, method: &str, from_version: u32) {
        self.migrations.push(MigrationEdgeAbi {
            method: method.to_owned(),
            from_version,
        });
    }

    /// Types come from the registry; methods and events are stable-sorted by
    /// name, which is what `validate_manifest` requires.
    #[must_use]
    pub fn finish(self) -> Manifest {
        let mut methods = self.methods;
        methods.sort_by(|a, b| a.name.cmp(&b.name));
        let mut events = self.events;
        events.sort_by(|a, b| a.name.cmp(&b.name));

        Manifest {
            schema_version: "wasm-abi/1".to_owned(),
            types: self.registry.into_types(),
            methods,
            events,
            state_root: self.state_root,
            state_version: self.state_version,
            migrations: self.migrations,
        }
    }
}
