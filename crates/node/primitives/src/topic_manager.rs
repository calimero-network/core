use std::collections::HashSet;
use std::sync::Arc;

use calimero_network_primitives::client::NetworkClient;
use libp2p::gossipsub::IdentTopic;
use tokio::sync::RwLock;
use tracing::{debug, info};

/// Tracks gossipsub topic subscriptions with deduplication.
#[derive(Clone, Debug)]
pub struct TopicManager {
    network_client: NetworkClient,
    subscribed: Arc<RwLock<HashSet<String>>>,
}

impl TopicManager {
    pub fn new(network_client: NetworkClient) -> Self {
        Self {
            network_client,
            subscribed: Arc::new(RwLock::new(HashSet::new())),
        }
    }

    /// Subscribe to a topic if not already subscribed.
    ///
    /// The set is held for the network call as well as the check, here and in
    /// [`Self::unsubscribe`]. Checked and updated around the call instead, a
    /// subscribe and an unsubscribe of one topic could interleave so the set
    /// keeps the topic while the swarm has dropped it - and every later call
    /// then skips the topic as already subscribed.
    pub async fn ensure_subscribed(&self, topic: &str) -> eyre::Result<()> {
        let mut subs = self.subscribed.write().await;
        if subs.contains(topic) {
            debug!(topic, "already subscribed, skipping");
            return Ok(());
        }
        let ident_topic = IdentTopic::new(topic);
        let _ignored = self.network_client.subscribe(ident_topic).await?;
        subs.insert(topic.to_owned());
        info!(topic, "subscribed to topic");
        Ok(())
    }

    /// Unsubscribe from a topic.
    pub async fn unsubscribe(&self, topic: &str) -> eyre::Result<()> {
        let mut subs = self.subscribed.write().await;
        let ident_topic = IdentTopic::new(topic);
        let _ignored = self.network_client.unsubscribe(ident_topic).await?;
        subs.remove(topic);
        info!(topic, "unsubscribed from topic");
        Ok(())
    }

    /// Check if subscribed to a topic.
    pub async fn is_subscribed(&self, topic: &str) -> bool {
        self.subscribed.read().await.contains(topic)
    }

    /// List all subscribed topics.
    pub async fn subscribed_topics(&self) -> Vec<String> {
        self.subscribed.read().await.iter().cloned().collect()
    }
}

#[cfg(test)]
mod tests {
    //! Under `#[actix::test]`, so the stub's mailbox is pumped by the runtime
    //! driving the manager's `.await`s.

    use std::collections::BTreeSet;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use actix::{Actor, AsyncContext, Context, Handler};
    use calimero_network_primitives::client::NetworkClient;
    use calimero_network_primitives::messages::NetworkMessage;
    use calimero_utils_actix::LazyRecipient;
    use tokio::sync::{mpsc, Semaphore};

    use super::TopicManager;

    /// Applies a subscribe or an unsubscribe the moment it arrives, as the swarm
    /// does, but answers a subscribe only once `reply` hands out a permit - the
    /// window between the swarm acting and the caller hearing back.
    struct SlowToAnswer {
        live: Arc<Mutex<BTreeSet<String>>>,
        reply: Arc<Semaphore>,
        arrived: mpsc::UnboundedSender<()>,
    }

    impl Actor for SlowToAnswer {
        type Context = Context<Self>;
    }

    impl Handler<NetworkMessage> for SlowToAnswer {
        type Result = ();

        fn handle(&mut self, msg: NetworkMessage, ctx: &mut Self::Context) {
            match msg {
                NetworkMessage::Subscribe { request, outcome } => {
                    let _ = self
                        .live
                        .lock()
                        .expect("live topics")
                        .insert(request.0.to_string());
                    let _ignored = self.arrived.send(());
                    let reply = Arc::clone(&self.reply);
                    let _handle = ctx.spawn(actix::fut::wrap_future(async move {
                        reply.acquire().await.expect("never closed").forget();
                        let _ignored = outcome.send(Ok(request.0));
                    }));
                }
                NetworkMessage::Unsubscribe { request, outcome } => {
                    let _ = self
                        .live
                        .lock()
                        .expect("live topics")
                        .remove(&request.0.to_string());
                    let _ignored = outcome.send(Ok(request.0));
                }
                _ => {}
            }
        }
    }

    /// An unsubscribe that lands while a subscribe of the same topic is waiting on
    /// its answer must not leave the topic recorded as held. It used to: the
    /// subscribe recorded it after the unsubscribe had already dropped it from the
    /// swarm, and every later subscribe then skipped it as already held - a
    /// namespace whose topic the node silently never heard from again.
    #[actix::test]
    async fn an_unsubscribe_racing_a_subscribe_never_wedges_the_topic() {
        let live = Arc::new(Mutex::new(BTreeSet::new()));
        let reply = Arc::new(Semaphore::new(0));
        let (arrived_tx, mut arrived) = mpsc::unbounded_channel();
        let recipient = LazyRecipient::<NetworkMessage>::new();
        let network = recipient.clone();
        let stub = SlowToAnswer {
            live: Arc::clone(&live),
            reply: Arc::clone(&reply),
            arrived: arrived_tx,
        };
        let _addr = SlowToAnswer::create(move |ctx| {
            assert!(recipient.init(ctx), "network recipient init");
            stub
        });
        let topics = TopicManager::new(NetworkClient::new(network));
        let topic = "ns/race";

        let subscribing = {
            let topics = topics.clone();
            actix::spawn(async move { topics.ensure_subscribed(topic).await })
        };
        arrived
            .recv()
            .await
            .expect("the subscribe reaches the swarm");
        let unsubscribing = {
            let topics = topics.clone();
            actix::spawn(async move { topics.unsubscribe(topic).await })
        };
        // As far as the unsubscribe can get while the subscribe waits.
        tokio::time::sleep(Duration::from_millis(100)).await;
        reply.add_permits(2);
        subscribing
            .await
            .expect("join")
            .expect("the subscribe is answered");
        unsubscribing
            .await
            .expect("join")
            .expect("the unsubscribe is answered");

        topics
            .ensure_subscribed(topic)
            .await
            .expect("the follow-up subscribe is answered");
        assert!(
            live.lock().expect("live topics").contains(topic),
            "a subscribe after the race has to reach the swarm, not be skipped as held"
        );
    }
}
