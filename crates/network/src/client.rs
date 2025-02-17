use eyre::Result as EyreResult;
use libp2p::gossipsub::{IdentTopic, MessageId, TopicHash};
use libp2p::{Multiaddr, PeerId};
use tokio::sync::mpsc;

use crate::stream::Stream;
use crate::Command;

// TODO: Delete probably and use Addr<NetworkManager> directly
#[derive(Clone, Debug)]
pub struct NetworkClient {
    pub(crate) _sender: mpsc::Sender<Command>,
}

impl NetworkClient {
    pub async fn listen_on(&self, _addr: Multiaddr) -> EyreResult<()> {
        todo!()
    }

    pub async fn bootstrap(&self) -> EyreResult<()> {
        todo!()
    }

    pub async fn subscribe(&self, _topic: IdentTopic) -> EyreResult<IdentTopic> {
        todo!()
    }

    pub async fn unsubscribe(&self, _topic: IdentTopic) -> EyreResult<IdentTopic> {
        todo!()
    }

    pub async fn open_stream(&self, _peer_id: PeerId) -> EyreResult<Stream> {
        todo!()
    }

    pub async fn peer_count(&self) -> usize {
        todo!()
    }

    pub async fn mesh_peer_count(&self, _topic: TopicHash) -> usize {
        todo!()
    }

    pub async fn mesh_peers(&self, _topic: TopicHash) -> Vec<PeerId> {
        todo!()
    }

    pub async fn publish(&self, _topic: TopicHash, _data: Vec<u8>) -> EyreResult<MessageId> {
        todo!()
    }

    pub async fn dial(&self, _peer_addr: Multiaddr) -> EyreResult<Option<()>> {
        todo!()
    }
}
