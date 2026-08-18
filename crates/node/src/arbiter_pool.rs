//! Arbiter pool for spawning Actix actors across multiple arbiters.
//!
//! **Why this exists**: Actix requires actors to run on arbiters, and we need
//! multiple arbiters to distribute actors across threads. This module encapsulates
//! the complex async machinery required to spawn and manage arbiters.
//!
//! **SRP Applied**: Arbiter management is separated from node startup logic.

use std::sync::Arc;

use actix::{Arbiter, System};
use eyre::{OptionExt, WrapErr};
use futures_util::{stream, StreamExt};
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;

/// Pool of Actix arbiters for spawning actors.
///
/// This manages the lifecycle of an Actix system and provides a simple interface
/// to get arbiter handles for starting actors.
///
/// # Architecture
///
/// - Spawns an Actix `System` in a blocking task (Actix requirement)
/// - System continuously generates new arbiters
/// - Arbiters are provided via a channel to the async runtime
/// - Pool provides a simple `get()` interface to retrieve arbiters
///
/// # Example
///
/// ```ignore
/// let mut pool = ArbiterPool::new().await?;
///
/// // Get arbiters for starting actors
/// let arb1 = pool.get().await?;
/// let arb2 = pool.get().await?;
///
/// Actor::start_in_arbiter(&arb1, |ctx| MyActor::new());
/// ```
pub struct ArbiterPool {
    /// Receiver for arbiter handles from the system
    stream: Arc<
        tokio::sync::Mutex<
            std::pin::Pin<Box<dyn futures_util::Stream<Item = actix::ArbiterHandle> + Send>>,
        >,
    >,

    /// Handle to the system task (public so caller can await it)
    pub system_handle: JoinHandle<eyre::Result<()>>,

    /// Handle used to stop the system from outside its own threads.
    ///
    /// Without this there is no way to end the system: `System::current()` only
    /// resolves on a thread the system is registered on, and every one of those
    /// is owned by the system itself. `System` is a cheap clonable handle whose
    /// `stop` just sends on a channel, so holding one here costs nothing and is
    /// the only thing that lets shutdown reach the arbiters.
    system: System,
}

impl ArbiterPool {
    /// Create a new arbiter pool.
    ///
    /// This spawns an Actix system in a blocking task and sets up the arbiter
    /// generation machinery.
    ///
    /// # Errors
    ///
    /// Returns error if the Actix system fails to start or if the initial
    /// arbiter cannot be retrieved.
    pub async fn new() -> eyre::Result<Self> {
        let (tx, mut rx) = mpsc::channel(1);
        let (sys_tx, sys_rx) = oneshot::channel();

        // Spawn Actix system in blocking task (Actix requires dedicated thread)
        let system_handle = tokio::task::spawn_blocking(move || {
            let system = System::new();

            // `System::new` registers itself on this thread, so this is the one
            // place a handle can be taken. Sent out before `run()` blocks,
            // because nothing after that line executes until the system stops.
            let _ignored = sys_tx.send(System::current());

            let _ignored = system.runtime().spawn({
                let task = async move {
                    let mut arb = Arbiter::current();

                    loop {
                        // Send current arbiter
                        tx.send(Some(arb)).await?;

                        // Send None signals to pace arbiter generation
                        // (allows consumer to catch up)
                        tx.send(None).await?;
                        tx.send(None).await?;

                        // Create next arbiter
                        arb = Arbiter::new().handle();
                    }
                };

                async {
                    let _ignored: eyre::Result<()> = task.await;
                    System::current().stop();
                }
            });

            system
                .run()
                .wrap_err("the actix subsystem ran into an error")
        });

        // Create stream that filters out None signals
        let stream = Box::pin(stream::poll_fn(move |cx| rx.poll_recv(cx)).filter_map(async |t| t));

        let system = sys_rx
            .await
            .wrap_err("the actix system stopped before it could be reached")?;

        Ok(Self {
            stream: Arc::new(tokio::sync::Mutex::new(stream)),
            system_handle,
            system,
        })
    }

    /// Stop the Actix system, and with it every arbiter thread and actor.
    ///
    /// Dropping the pool also stops the system — the arbiter-generation task
    /// calls `System::current().stop()` once its channel closes — but a drop
    /// only happens after the owner returns, which is too late: by then the
    /// global runtime is going away, and the arbiters tear their actors down
    /// against a runtime whose IO resources are already failing. That is where
    /// the mdns/netlink spin comes from. Calling this during shutdown brings the
    /// actors down while the runtime is still healthy.
    ///
    /// Returns immediately: `System::stop` only sends on a channel. Await
    /// `system_handle` to observe the system actually finishing.
    pub fn stop(&self) {
        self.system.stop();
    }

    /// Get an arbiter handle for starting an actor.
    ///
    /// This retrieves the next available arbiter from the pool.
    ///
    /// # Errors
    ///
    /// Returns error if no arbiter is available (system stopped).
    pub async fn get(&mut self) -> eyre::Result<actix::ArbiterHandle> {
        let mut stream = self.stream.lock().await;
        stream.next().await.ok_or_eyre("failed to get arbiter")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_arbiter_pool_creation() {
        let pool = ArbiterPool::new().await;
        assert!(pool.is_ok(), "Failed to create arbiter pool");
    }

    /// Shutdown depends on being able to end the system *before* the owner
    /// returns. Relying on the drop path instead brought the actors down against
    /// a runtime that was already going away, which is what let the swarm's mdns
    /// watcher spin on a dead netlink socket until the process was killed.
    #[tokio::test]
    async fn stop_ends_the_system_while_the_pool_is_still_alive() {
        let mut pool = ArbiterPool::new().await.expect("pool must start");
        let _arbiter = pool.get().await.expect("an arbiter must be available");

        pool.stop();

        // The system task must finish on its own, with the pool still in scope —
        // i.e. without the drop that used to be the only thing stopping it.
        let stopped =
            tokio::time::timeout(std::time::Duration::from_secs(10), &mut pool.system_handle).await;

        assert!(
            stopped.is_ok(),
            "stop() must end the system without waiting for the pool to drop"
        );
        assert!(
            stopped.expect("not timed out").is_ok(),
            "the system task must not panic on stop"
        );
    }

    #[tokio::test]
    async fn test_get_multiple_arbiters() {
        let mut pool = ArbiterPool::new().await.unwrap();

        // Should be able to get multiple arbiters
        let arb1 = pool.get().await;
        let arb2 = pool.get().await;

        assert!(arb1.is_ok(), "Failed to get first arbiter");
        assert!(arb2.is_ok(), "Failed to get second arbiter");
    }
}
