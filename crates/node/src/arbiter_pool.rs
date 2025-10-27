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
use tokio::sync::mpsc;
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

        // Spawn Actix system in blocking task (Actix requires dedicated thread)
        let system_handle = tokio::task::spawn_blocking(move || {
            let system = System::new();

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

        Ok(Self {
            stream: Arc::new(tokio::sync::Mutex::new(stream)),
            system_handle,
        })
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
