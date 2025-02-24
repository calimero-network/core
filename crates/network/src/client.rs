use actix::Addr;
use eyre::Result as EyreResult;
use libp2p::gossipsub::{IdentTopic, MessageId, TopicHash};
use libp2p::{Multiaddr, PeerId};

use crate::handler::command::bootstrap::Bootstrap;
use crate::handler::command::dial::Dial;
use crate::handler::command::listen::ListenOn;
use crate::handler::command::mesh_peer_count::MeshPeerCount;
use crate::handler::command::mesh_peers::MeshPeers;
use crate::handler::command::open_stream::OpenStream;
use crate::handler::command::peer_count::PeerCount;
use crate::handler::command::publish::Publish;
use crate::handler::command::subscribe::Subscribe;
use crate::handler::command::unsubscribe::Unsubscribe;
use crate::stream::Stream;
use crate::NetworkManager;

// TODO: Probably just use network_manager addr directly and delete this client.
#[derive(Clone, Debug)]
pub struct NetworkClient {
    network_manager: Addr<NetworkManager>,
}

impl NetworkClient {
    pub(crate) const fn new(network_manager: Addr<NetworkManager>) -> Self {
        Self { network_manager }
    }

    pub async fn dial(&self, peer_addr: Multiaddr) -> EyreResult<Option<()>> {
        self.network_manager
            .send(Dial::from(peer_addr))
            .await
            .expect("Mailbox not to be dropped")
    }

    pub async fn listen_on(&self, addr: Multiaddr) -> EyreResult<()> {
        self.network_manager
            .send(ListenOn::from(addr))
            .await
            .expect("Mailbox not to be dropped")
    }

    pub async fn bootstrap(&self) -> EyreResult<()> {
        let _result = self
            .network_manager
            .send(Bootstrap)
            .await
            .expect("Mailbox not to be dropped")?;
        Ok(())
    }

    pub async fn subscribe(&self, topic: IdentTopic) -> EyreResult<IdentTopic> {
        self.network_manager
            .send(Subscribe::from(topic))
            .await
            .expect("Mailbox not to be dropped")
    }

    pub async fn unsubscribe(&self, topic: IdentTopic) -> EyreResult<IdentTopic> {
        self.network_manager
            .send(Unsubscribe::from(topic))
            .await
            .expect("Mailbox not to be dropped")
    }

    pub async fn publish(&self, topic: TopicHash, data: Vec<u8>) -> EyreResult<MessageId> {
        self.network_manager
            .send(Publish::from((topic, data)))
            .await?
    }

    pub async fn open_stream(&self, peer_id: PeerId) -> EyreResult<Stream> {
        self.network_manager
            .send(OpenStream::from(peer_id))
            .await
            .expect("Mailbox not to be dropped")
    }

    pub async fn peer_count(&self) -> usize {
        self.network_manager
            .send(PeerCount)
            .await
            .expect("Mailbox not to be dropped")
    }

    pub async fn mesh_peer_count(&self, topic: TopicHash) -> usize {
        self.network_manager
            .send(MeshPeerCount::from(topic))
            .await
            .expect("Mailbox not to be dropped")
    }

    pub async fn mesh_peers(&self, topic: TopicHash) -> Vec<PeerId> {
        self.network_manager
            .send(MeshPeers::from(topic))
            .await
            .expect("Mailbox not to be dropped")
    }
}
