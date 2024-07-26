use std::collections::HashSet;
use std::sync::Arc;

use calimero_identity::IdentityHandler;
use libp2p::{gossipsub, Multiaddr, PeerId};
use tokio::sync::{mpsc, oneshot, RwLock};

use crate::{config, stream, Command};

#[derive(Clone)]
pub struct NetworkClient {
    pub catchup_config: config::CatchupConfig,
    pub(crate) sender: mpsc::Sender<Command>,
    pub identity_handler: Option<Arc<RwLock<IdentityHandler>>>,
}

impl NetworkClient {
    pub async fn listen_on(&self, addr: Multiaddr) -> eyre::Result<()> {
        let (sender, receiver) = oneshot::channel();

        self.sender
            .send(Command::ListenOn { addr, sender })
            .await
            .expect("Command receiver not to be dropped.");

        receiver.await.expect("Sender not to be dropped.")
    }

    pub async fn bootstrap(&self) -> eyre::Result<()> {
        let (sender, receiver) = oneshot::channel();

        self.sender
            .send(Command::Bootstrap { sender })
            .await
            .expect("Command receiver not to be dropped.");

        receiver.await.expect("Sender not to be dropped.")?;

        Ok(())
    }

    pub async fn subscribe(
        &self,
        topic: gossipsub::IdentTopic,
    ) -> eyre::Result<gossipsub::IdentTopic> {
        let (sender, receiver) = oneshot::channel();

        self.sender
            .send(Command::Subscribe { topic, sender })
            .await
            .expect("Command receiver not to be dropped.");

        receiver.await.expect("Sender not to be dropped.")
    }

    pub async fn unsubscribe(
        &self,
        topic: gossipsub::IdentTopic,
    ) -> eyre::Result<gossipsub::IdentTopic> {
        let (sender, receiver) = oneshot::channel();

        self.sender
            .send(Command::Unsubscribe { topic, sender })
            .await
            .expect("Command receiver not to be dropped.");

        receiver.await.expect("Sender not to be dropped.")
    }

    pub async fn open_stream(&self, peer_id: PeerId) -> eyre::Result<stream::Stream> {
        let (sender, receiver) = oneshot::channel();

        self.sender
            .send(Command::OpenStream { peer_id, sender })
            .await
            .expect("Command receiver not to be dropped.");

        receiver.await.expect("Sender not to be dropped.")
    }

    pub async fn peer_count(&self) -> usize {
        let (sender, receiver) = oneshot::channel();

        self.sender
            .send(Command::PeerCount { sender })
            .await
            .expect("Command receiver not to be dropped.");

        receiver.await.expect("Sender not to be dropped.")
    }

    pub async fn mesh_peer_count(&self, topic: gossipsub::TopicHash) -> usize {
        let (sender, receiver) = oneshot::channel();

        self.sender
            .send(Command::MeshPeerCount { topic, sender })
            .await
            .expect("Command receiver not to be dropped.");

        receiver.await.expect("Sender not to be dropped.")
    }

    pub async fn publish(
        &self,
        topic: gossipsub::TopicHash,
        data: Vec<u8>,
    ) -> eyre::Result<gossipsub::MessageId> {
        let (sender, receiver) = oneshot::channel();

        self.sender
            .send(Command::Publish {
                topic,
                data,
                sender,
            })
            .await
            .expect("Command receiver not to be dropped.");

        receiver.await.expect("Sender not to be dropped.")
    }

    pub async fn dial(&self, peer_addr: Multiaddr) -> eyre::Result<Option<()>> {
        let (sender, receiver) = oneshot::channel();

        self.sender
            .send(Command::Dial { peer_addr, sender })
            .await
            .expect("Command receiver not to be dropped.");

        receiver.await.expect("Sender not to be dropped.")
    }

    pub async fn start_providing(&self, key: String) {
        let (sender, receiver) = oneshot::channel();

        self.sender
            .send(Command::StartProviding { key, sender })
            .await
            .expect("Command receiver not to be dropped.");

        receiver.await.expect("Sender not to be dropped.");
    }

    pub async fn get_providers(&self, key: String) -> HashSet<PeerId> {
        let (sender, receiver) = oneshot::channel();

        self.sender
            .send(Command::GetProviders { key, sender })
            .await
            .expect("Command receiver not to be dropped.");

        receiver.await.expect("Sender not to be dropped.")
    }

    pub fn set_identity_handler(&mut self, handler: Arc<RwLock<IdentityHandler>>) {
        self.identity_handler = Some(handler);
    }
}
