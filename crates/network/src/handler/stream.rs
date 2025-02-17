use actix::{Context, Handler, Message, StreamHandler};
use libp2p::swarm::SwarmEvent;
use libp2p::{PeerId, Stream as P2pStream};

use crate::{BehaviourEvent, EventLoop};

pub mod incoming;
pub mod swarm;

use incoming::FromIncoming;
use swarm::FromSwarm;

#[derive(Message)]
#[rtype(result = "()")]
pub enum FromStreamInner {
    Started,
    Finished,
    Value(SwarmEvent<BehaviourEvent>),
}

#[derive(Message)]
#[rtype(result = "()")]
pub enum FromStreamInner2 {
    Started,
    Finished,
    Value(FromStream),
}

pub enum FromStream {
    Swarm(SwarmEvent<BehaviourEvent>),
    Incoming(Option<(PeerId, P2pStream)>),
}

impl Handler<FromStreamInner> for EventLoop {
    type Result = ();

    fn handle(&mut self, msg: FromStreamInner, ctx: &mut Context<Self>) -> Self::Result {
        match msg {
            FromStreamInner::Started => StreamHandler::<FromSwarm>::started(self, ctx),
            FromStreamInner::Finished => StreamHandler::<FromSwarm>::finished(self, ctx),
            FromStreamInner::Value(event) => {
                StreamHandler::<FromSwarm>::handle(self, event.into(), ctx)
            }
        }
    }
}

#[derive(Message)]
#[rtype(result = "()")]
pub enum FromIncomingStream {
    Started,
    Finished,
    Value(Option<(PeerId, P2pStream)>),
}

impl Handler<FromIncomingStream> for EventLoop {
    type Result = ();

    fn handle(&mut self, msg: FromIncomingStream, ctx: &mut Context<Self>) -> Self::Result {
        match msg {
            FromIncomingStream::Started => StreamHandler::<FromIncoming>::started(self, ctx),
            FromIncomingStream::Finished => StreamHandler::<FromIncoming>::finished(self, ctx),
            FromIncomingStream::Value(event) => {
                StreamHandler::<FromIncoming>::handle(self, event.into(), ctx)
            }
        }
    }
}
