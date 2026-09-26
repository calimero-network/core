//! Compile an installed application's modules before anything needs them.
//!
//! A module is otherwise compiled on first use, so the first context created
//! from a newly installed application — and the request that creates it — pays
//! for compiling its WASM: seconds in a debug build, and not free in a release
//! one. Installing warms the same cache [`ContextManager::get_module_for_blob`]
//! fills, so that cost moves to the install. The cache is bounded, so on a node
//! running many applications a warmed module can still be evicted before use;
//! then it is compiled on first use as before.

use actix::{ActorFutureExt, ActorResponse, ActorTryFutureExt, Handler, Message, WrapFuture};
use calimero_context_client::messages::PrecompileApplicationRequest;
use eyre::{bail, OptionExt};

use crate::ContextManager;

impl Handler<PrecompileApplicationRequest> for ContextManager {
    type Result = ActorResponse<Self, <PrecompileApplicationRequest as Message>::Result>;

    fn handle(
        &mut self,
        PrecompileApplicationRequest { application_id }: PrecompileApplicationRequest,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        let node_client = self.node_client.clone();
        let services = async move {
            let Some(application) = node_client.get_application(&application_id)? else {
                bail!("application {application_id} is not installed");
            };
            let blob_id = application.blob.bytecode;
            // Exactly the `service_name` values contexts load the blob under:
            // one per service of a bundle, or `None` for a single wasm.
            let services = node_client
                .bundle_service_names(&blob_id)
                .await?
                .ok_or_eyre("the application's bytecode blob is not on this node")?;
            Ok((blob_id, services))
        }
        .into_actor(self);

        let compiled = services.and_then(|(blob_id, services), act, _ctx| {
            let missing: Vec<_> = services
                .into_iter()
                .filter(|service| !act.modules.contains_key(&(blob_id, service.clone())))
                .collect();
            let count = missing.len();
            // One at a time: each compile already runs on the blocking pool, and
            // running them together would only contend for the same cores.
            let mut chain = actix::fut::ready(Ok(())).boxed_local();
            for service in missing {
                chain = chain
                    .and_then(move |(), act: &mut Self, _ctx| {
                        act.get_module_for_blob(blob_id, service)
                            .map_ok(|_module, _act, _ctx| ())
                    })
                    .boxed_local();
            }
            chain.map_ok(move |(), _act, _ctx| count)
        });

        ActorResponse::r#async(compiled)
    }
}

#[cfg(test)]
mod tests {
    use futures_util::io::Cursor;

    use calimero_primitives::application::ApplicationId;
    use calimero_store::db::InMemoryDB;
    use calimero_store::{key, types, Store};

    use super::*;
    use crate::test_support::actor::over;

    /// Compiles run on the node's global runtime, which must be multi-threaded;
    /// `actix::test` runs on a current-thread one, so start one beside it.
    fn global_runtime() {
        static RUNTIME: std::sync::OnceLock<tokio::runtime::Runtime> = std::sync::OnceLock::new();
        let runtime = RUNTIME.get_or_init(|| {
            tokio::runtime::Builder::new_multi_thread()
                .worker_threads(1)
                .enable_all()
                .build()
                .expect("a multi-threaded runtime")
        });
        // Already initialised by an earlier test in this process is fine.
        let _ = std::thread::scope(|scope| {
            scope
                .spawn(|| runtime.block_on(async { calimero_utils_actix::init_global_runtime() }))
                .join()
        });
    }

    /// The smallest module the runtime accepts: it only exports its memory.
    const WASM: &[u8] = b"\0asm\x01\0\0\0\x05\x03\x01\x00\x01\x07\x0a\x01\x06memory\x02\x00";

    #[actix::test]
    async fn precompiling_fills_the_module_cache_once() {
        global_runtime();
        let store = Store::new(std::sync::Arc::new(InMemoryDB::owned()));
        calimero_governance_store::NodeDeviceRepository::new(&store)
            .provision_account_root()
            .expect("provision the account root an initialised node has");
        let harness = over(store.clone()).await;

        let (blob_id, size) = harness
            .node_client
            .add_blob(Cursor::new(WASM), Some(WASM.len() as u64), None)
            .await
            .expect("store the wasm");
        let application_id = ApplicationId::from([7; 32]);
        store
            .handle()
            .put(
                &key::ApplicationMeta::new(application_id),
                &types::ApplicationMeta::new(
                    key::BlobMeta::new(blob_id),
                    size,
                    "file://test.wasm".into(),
                    vec![].into(),
                    key::BlobMeta::new([0; 32].into()),
                    types::PackageInfo {
                        package: "com.test.app".into(),
                        version: "1.0.0".into(),
                        signer_id: "did:key:test".into(),
                        state_version: 0,
                    },
                ),
            )
            .expect("install the application");

        let request = PrecompileApplicationRequest { application_id };
        let first = harness.manager.send(request).await.expect("mailbox");
        assert_eq!(first.expect("precompiles"), 1, "the one module is compiled");
        let second = harness.manager.send(request).await.expect("mailbox");
        assert_eq!(
            second.expect("precompiles"),
            0,
            "and cached, so not compiled again"
        );
    }

    #[actix::test]
    async fn precompiling_an_application_not_installed_fails() {
        let store = Store::new(std::sync::Arc::new(InMemoryDB::owned()));
        let harness = over(store).await;
        let outcome = harness
            .manager
            .send(PrecompileApplicationRequest {
                application_id: ApplicationId::from([9; 32]),
            })
            .await
            .expect("mailbox");
        assert!(outcome.is_err());
    }
}
