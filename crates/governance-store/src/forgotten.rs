//! The namespaces the user removed from this node, node-local and never synced:
//! no automatic path follows one, and only an explicit join clears it.

use calimero_context_config::types::ContextGroupId;
use calimero_primitives::application::ApplicationId;
use calimero_store::key::{
    NamespaceForgotten as Key, NAMESPACE_FORGOTTEN_KEY_LEN, NAMESPACE_FORGOTTEN_PREFIX,
};
use calimero_store::types::NamespaceForgotten as Value;
use calimero_store::Store;
use eyre::Result as EyreResult;

/// One removed namespace, as listed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ForgottenNamespace {
    pub namespace_id: ContextGroupId,
    pub application_id: Option<ApplicationId>,
    pub forgotten_at_ms: u64,
}

/// Typed repository for the removed-namespace records.
pub struct ForgottenNamespaces<'a> {
    store: &'a Store,
}

impl<'a> ForgottenNamespaces<'a> {
    pub fn new(store: &'a Store) -> Self {
        Self { store }
    }

    /// Record that `namespace` was removed from this node. Idempotent; the
    /// latest call's application and time stand.
    pub fn mark(
        &self,
        namespace: &ContextGroupId,
        application_id: Option<ApplicationId>,
        forgotten_at_ms: u64,
    ) -> EyreResult<()> {
        self.store.handle().put(
            &Key::new(namespace.to_bytes()),
            &Value {
                application_id: application_id.map(|app| *app),
                forgotten_at_ms,
            },
        )?;
        Ok(())
    }

    /// Whether `namespace` was removed from this node and not joined again since.
    pub fn is_forgotten(&self, namespace: &ContextGroupId) -> EyreResult<bool> {
        Ok(self.store.handle().has(&Key::new(namespace.to_bytes()))?)
    }

    /// Lift the record. Only an explicit join by the user may call this.
    pub fn clear(&self, namespace: &ContextGroupId) -> EyreResult<()> {
        self.store
            .handle()
            .delete(&Key::new(namespace.to_bytes()))?;
        Ok(())
    }

    /// Every removed namespace, in id order.
    pub fn list(&self) -> EyreResult<Vec<ForgottenNamespace>> {
        let prefix = [NAMESPACE_FORGOTTEN_PREFIX];
        let upper = [NAMESPACE_FORGOTTEN_PREFIX + 1];
        let mut out = Vec::new();
        for (key, value) in self.store.raw_scan(
            calimero_store::db::Column::ContextLocal,
            &prefix,
            &upper,
            None,
        )? {
            // A 64-byte left-marker key can start with the same byte.
            if key.len() != NAMESPACE_FORGOTTEN_KEY_LEN {
                continue;
            }
            let value: Value = borsh::from_slice(&value)?;
            let mut id = [0; 32];
            id.copy_from_slice(&key[1..]);
            out.push(ForgottenNamespace {
                namespace_id: ContextGroupId::from(id),
                application_id: value.application_id.map(ApplicationId::from),
                forgotten_at_ms: value.forgotten_at_ms,
            });
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use calimero_store::key::ContextLeftMarker;
    use calimero_store::types::ContextLeftMarker as LeftValue;

    use super::*;
    use crate::test_fixtures::test_store;

    const NS_A: [u8; 32] = [0x21; 32];
    const NS_B: [u8; 32] = [0x22; 32];
    const APP: [u8; 32] = [0x77; 32];

    #[test]
    fn a_marked_namespace_is_forgotten_until_cleared() {
        let store = test_store();
        let forgotten = ForgottenNamespaces::new(&store);
        let ns = ContextGroupId::from(NS_A);

        assert!(!forgotten.is_forgotten(&ns).unwrap());
        forgotten.mark(&ns, None, 5).unwrap();
        assert!(forgotten.is_forgotten(&ns).unwrap());
        assert!(!forgotten.is_forgotten(&ContextGroupId::from(NS_B)).unwrap());

        forgotten.clear(&ns).unwrap();
        assert!(!forgotten.is_forgotten(&ns).unwrap());
    }

    #[test]
    fn listing_returns_each_marker_with_its_application_and_time() {
        let store = test_store();
        let forgotten = ForgottenNamespaces::new(&store);
        let (a, b) = (ContextGroupId::from(NS_A), ContextGroupId::from(NS_B));

        forgotten.mark(&b, None, 9).unwrap();
        forgotten
            .mark(&a, Some(ApplicationId::from(APP)), 7)
            .unwrap();

        assert_eq!(
            forgotten.list().unwrap(),
            vec![
                ForgottenNamespace {
                    namespace_id: a,
                    application_id: Some(ApplicationId::from(APP)),
                    forgotten_at_ms: 7,
                },
                ForgottenNamespace {
                    namespace_id: b,
                    application_id: None,
                    forgotten_at_ms: 9,
                },
            ]
        );
    }

    /// The marker shares `ContextLocal` with the 64-byte left marker, whose
    /// context id may begin with the marker's own prefix byte.
    #[test]
    fn a_left_marker_is_neither_listed_nor_mistaken_for_a_forgotten_namespace() {
        let store = test_store();
        let forgotten = ForgottenNamespaces::new(&store);
        let context =
            calimero_primitives::context::ContextId::from([NAMESPACE_FORGOTTEN_PREFIX; 32]);
        let member = calimero_primitives::identity::PublicKey::from([0x33; 32]);
        store
            .handle()
            .put(
                &ContextLeftMarker::new(context, member),
                &LeftValue { left_at_ms: 1 },
            )
            .unwrap();

        assert!(forgotten.list().unwrap().is_empty());
        assert!(!forgotten
            .is_forgotten(&ContextGroupId::from([NAMESPACE_FORGOTTEN_PREFIX; 32]))
            .unwrap());

        forgotten
            .mark(&ContextGroupId::from(NS_A), None, 2)
            .unwrap();
        assert_eq!(forgotten.list().unwrap().len(), 1);
    }
}
