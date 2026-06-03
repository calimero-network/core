use actix::{ActorResponse, Handler, Message, WrapFuture};
use calimero_context_client::messages::AcquireContextLockRequest;
use calimero_context_client::ContextAtomicKey;
use either::Either;
use tracing::error;

use crate::ContextManager;

/// Hand out the per-context execution lock guard to an off-actor caller.
///
/// This is the seam that lets the sync session serialize its host-side
/// storage mutations against the executor. The executor (`ExecuteRequest`)
/// takes `ContextMeta::lock()` for the whole of a WASM run; the sync
/// session's `EntityPush` / `EntityDeletePush` apply paths run in a
/// separate actor and historically wrote storage directly, holding only
/// the byte-level `index_mutation_guard`. That guard makes each individual
/// mutator atomic but does NOT make a whole multi-leaf apply atomic against
/// a concurrent delta merge — the two interleave their ancestor-hash
/// recomputes and record a torn root hash that delta-sync can't repair.
/// Handing the *same* `Arc<Mutex<ContextId>>` guard to the sync session
/// closes that gap.
///
/// Awaiting the lock here yields cooperatively (it's a `tokio::sync::Mutex`),
/// so an in-flight `ExecuteRequest` future on this actor keeps making
/// progress and drops its guard normally; this future then acquires it.
impl Handler<AcquireContextLockRequest> for ContextManager {
    type Result = ActorResponse<Self, <AcquireContextLockRequest as Message>::Result>;

    fn handle(
        &mut self,
        AcquireContextLockRequest { context }: AcquireContextLockRequest,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        // `lock()` clones the context's `Arc<Mutex<_>>` into an owned guard
        // (or a `'static` acquire future), so the returned `Either` does not
        // borrow `self` and the async block below can `into_actor(self)`.
        let lock = match self.get_or_fetch_context(&context) {
            Ok(Some(context_meta)) => context_meta.lock(),
            Ok(None) => return ActorResponse::reply(None),
            Err(err) => {
                error!(%context, %err, "acquire_context_lock: failed to fetch context");
                return ActorResponse::reply(None);
            }
        };

        ActorResponse::r#async(
            async move {
                let guard = match lock {
                    Either::Left(guard) => guard,
                    Either::Right(task) => task.await,
                };
                Some(ContextAtomicKey(guard))
            }
            .into_actor(self),
        )
    }
}
