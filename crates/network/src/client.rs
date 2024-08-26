use std::collections::HashSet;

use calimero_primitives::identity::PeerId;
use eyre::Result as EyreResult;
use libp2p::gossipsub::{IdentTopic, MessageId, TopicHash};
use libp2p::Multiaddr;
use tokio::sync::{mpsc, oneshot};

use crate::config::CatchupConfig;
use crate::stream::Stream;
use crate::Command;

#[derive(Clone, Debug)]
pub struct NetworkClient {
    pub catchup_config: CatchupConfig,
    pub(crate) sender: mpsc::Sender<Command>,
}

impl NetworkClient {
    pub async fn listen_on(&self, addr: Multiaddr) -> EyreResult<()> {
        let (sender, receiver) = oneshot::channel();

        self.sender
            .send(Command::ListenOn { addr, sender })
            .await
            .expect("Command receiver not to be dropped.");

        receiver.await.expect("Sender not to be dropped.")
    }

    pub async fn dial(&self, peer_addr: Multiaddr) -> EyreResult<Option<()>> {
        let (sender, receiver) = oneshot::channel();

        self.sender
            .send(Command::Dial { peer_addr, sender })
            .await
            .expect("Command receiver not to be dropped.");

        receiver.await.expect("Sender not to be dropped.")
    }

    pub async fn bootstrap(&self) -> EyreResult<()> {
        let (sender, receiver) = oneshot::channel();

        self.sender
            .send(Command::Bootstrap { sender })
            .await
            .expect("Command receiver not to be dropped.");

        let _ = receiver.await.expect("Sender not to be dropped.")?;

        Ok(())
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

    pub async fn subscribe(&self, topic: IdentTopic) -> EyreResult<IdentTopic> {
        let (sender, receiver) = oneshot::channel();

        self.sender
            .send(Command::Subscribe { topic, sender })
            .await
            .expect("Command receiver not to be dropped.");

        receiver.await.expect("Sender not to be dropped.")
    }

    pub async fn unsubscribe(&self, topic: IdentTopic) -> EyreResult<IdentTopic> {
        let (sender, receiver) = oneshot::channel();

        self.sender
            .send(Command::Unsubscribe { topic, sender })
            .await
            .expect("Command receiver not to be dropped.");

        receiver.await.expect("Sender not to be dropped.")
    }

    pub async fn open_stream(&self, peer_id: PeerId) -> EyreResult<Stream> {
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

    pub async fn mesh_peer_count(&self, topic: TopicHash) -> usize {
        let (sender, receiver) = oneshot::channel();

        self.sender
            .send(Command::MeshPeerCount { topic, sender })
            .await
            .expect("Command receiver not to be dropped.");

        receiver.await.expect("Sender not to be dropped.")
    }

    pub async fn mesh_peers(&self, topic: TopicHash) -> Vec<PeerId> {
        let (sender, receiver) = oneshot::channel();

        self.sender
            .send(Command::MeshPeers { topic, sender })
            .await
            .expect("Command receiver not to be dropped.");

        receiver.await.expect("Sender not to be dropped.")
    }

    pub async fn publish(&self, topic: TopicHash, data: Vec<u8>) -> EyreResult<MessageId> {
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
}
